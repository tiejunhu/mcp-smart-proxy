use std::collections::BTreeMap;
use std::ffi::OsString;

pub fn collect_env_var_names(value: &str) -> Vec<String> {
    let mut env_vars = Vec::new();
    let mut remaining = value;

    while let Some(start) = remaining.find("{env:") {
        let suffix = &remaining[start + 5..];
        let Some(end) = suffix.find('}') else {
            break;
        };
        let name = &suffix[..end];
        if !name.is_empty() && !env_vars.iter().any(|existing| existing == name) {
            env_vars.push(name.to_string());
        }
        remaining = &suffix[end + 1..];
    }

    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find('}') else {
            break;
        };
        let expression = &after_start[..end];
        let name = expression
            .split_once(":-")
            .map(|(name, _)| name)
            .unwrap_or(expression);
        if !name.is_empty() && !env_vars.iter().any(|existing| existing == name) {
            env_vars.push(name.to_string());
        }
        remaining = &after_start[end + 1..];
    }

    env_vars
}

pub fn render_env_placeholders(
    value: &str,
    env_values: &BTreeMap<String, OsString>,
    missing_value: &mut impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut rendered = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if value[index..].starts_with("{env:") {
            let suffix = &value[index + 5..];
            let Some(end) = suffix.find('}') else {
                rendered.push_str(&value[index..]);
                break;
            };
            let name = &suffix[..end];
            if name.is_empty() {
                rendered.push_str(&value[index..index + 6 + end]);
            } else if let Some(resolved) = env_values.get(name) {
                rendered.push_str(&resolved.to_string_lossy());
            } else {
                rendered.push_str(&missing_value(name)?);
            }
            index += 6 + end;
            continue;
        }

        if value[index..].starts_with("${") {
            let suffix = &value[index + 2..];
            let Some(end) = suffix.find('}') else {
                rendered.push_str(&value[index..]);
                break;
            };
            let expression = &suffix[..end];
            let (name, fallback) = expression
                .split_once(":-")
                .map(|(name, fallback)| (name, Some(fallback)))
                .unwrap_or((expression, None));
            if name.is_empty() {
                rendered.push_str(&value[index..index + 3 + end]);
            } else if let Some(resolved) = env_values.get(name) {
                rendered.push_str(&resolved.to_string_lossy());
            } else if let Some(fallback) = fallback {
                rendered.push_str(fallback);
            } else {
                rendered.push_str(&missing_value(name)?);
            }
            index += 3 + end;
            continue;
        }

        rendered.push(bytes[index] as char);
        index += 1;
    }

    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_both_placeholder_styles() {
        assert_eq!(
            collect_env_var_names("Bearer {env:DEMO_TOKEN} ${DEMO_REGION:-global} ${TEAM}"),
            vec![
                "DEMO_TOKEN".to_string(),
                "DEMO_REGION".to_string(),
                "TEAM".to_string()
            ]
        );
    }

    #[test]
    fn renders_placeholders_from_environment_with_fallbacks() {
        let env_values = BTreeMap::from([
            ("DEMO_TOKEN".to_string(), OsString::from("abc")),
            ("TEAM".to_string(), OsString::from("infra")),
        ]);
        let rendered = render_env_placeholders(
            "Bearer {env:DEMO_TOKEN} ${DEMO_REGION:-global} ${TEAM}",
            &env_values,
            &mut |name| Err(format!("missing {name}")),
        )
        .unwrap();

        assert_eq!(rendered, "Bearer abc global infra");
    }

    #[test]
    fn returns_custom_error_for_missing_values_without_fallback() {
        let error = render_env_placeholders("${TEAM}", &BTreeMap::new(), &mut |name| {
            Err(format!("missing {name}"))
        })
        .unwrap_err();

        assert_eq!(error, "missing TEAM");
    }
}
