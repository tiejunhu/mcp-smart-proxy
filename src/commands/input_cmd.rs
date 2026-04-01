use std::error::Error;
use std::io::{self, Write};

use crate::console::{operation_error, print_app_event};
use crate::input_popup::{
    PopupInputRequest, PopupOption, PopupQuestion, read_popup_request_from_stdin, show_popup_dialog,
};

pub(super) fn run_input_test_command() -> Result<(), Box<dyn Error>> {
    let response = show_popup_dialog(sample_request()).map_err(|error| {
        operation_error(
            "cli.input.test",
            "failed to show the popup input test dialog",
            error,
        )
    })?;
    let rendered_response = serde_json::to_string_pretty(&response).map_err(|error| {
        operation_error(
            "cli.input.test.render",
            "failed to render the popup input response as JSON",
            Box::new(error),
        )
    })?;
    eprintln!(
        "[input-popup-debug] input test received response: {:?}",
        response
    );
    print_app_event("cli.input.test", "Popup response JSON:");
    println!("{rendered_response}");
    io::stdout().flush().map_err(|error| {
        operation_error(
            "cli.input.test.flush",
            "failed to flush the popup input test output",
            Box::new(error),
        )
    })?;
    Ok(())
}

pub(super) fn run_input_popup_command() -> Result<(), Box<dyn Error>> {
    let request = read_popup_request_from_stdin().map_err(|error| {
        operation_error(
            "cli.input.popup.read",
            "failed to read popup input JSON request from stdin",
            error,
        )
    })?;
    let response = show_popup_dialog(request).map_err(|error| {
        operation_error(
            "cli.input.popup.show",
            "failed to show the popup input dialog",
            error,
        )
    })?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn sample_request() -> PopupInputRequest {
    PopupInputRequest {
        questions: vec![
            PopupQuestion {
                id: "delivery_strategy".to_string(),
                question: "这次运行应当采用哪种交付策略？".to_string(),
                options: vec![
                    PopupOption {
                        label: "快速路径".to_string(),
                        description: "优先选择最快方案，并接受较窄的检查范围。".to_string(),
                    },
                    PopupOption {
                        label: "平衡方案".to_string(),
                        description: "用少量速度换取更稳妥的校验和默认行为。".to_string(),
                    },
                ],
            },
            PopupQuestion {
                id: "summary_style".to_string(),
                question: "最终结果应当如何总结？".to_string(),
                options: vec![
                    PopupOption {
                        label: "简短说明".to_string(),
                        description: "返回一段紧凑的说明，只保留最关键的信息。".to_string(),
                    },
                    PopupOption {
                        label: "检查清单".to_string(),
                        description: "返回便于快速浏览的扁平条目列表。".to_string(),
                    },
                ],
            },
        ],
    }
}
