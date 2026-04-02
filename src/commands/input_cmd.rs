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
                question: "Which delivery strategy\nshould this run use?".to_string(),
                options: vec![
                    PopupOption {
                        label: "Fast path".to_string(),
                        description:
                            "Prefer the quickest option and accept a narrower validation scope."
                                .to_string(),
                    },
                    PopupOption {
                        label: "Balanced".to_string(),
                        description: "Trade a little speed for safer checks and steadier defaults."
                            .to_string(),
                    },
                ],
            },
            PopupQuestion {
                id: "summary_style".to_string(),
                question: "How should the final result be summarized?".to_string(),
                options: vec![
                    PopupOption {
                        label: "Short note".to_string(),
                        description:
                            "Return one compact explanation with only the most important details."
                                .to_string(),
                    },
                    PopupOption {
                        label: "Checklist".to_string(),
                        description: "Return a flat list that is easy to scan quickly.".to_string(),
                    },
                ],
            },
        ],
    }
}
