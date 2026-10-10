//! Editor command parsing regressions for `parakit config edit`.

use super::*;

#[test]
fn editor_command_prefers_visual_and_preserves_arguments() -> Result<()> {
    let editor = configured_editor(
        Some(r#""Visual Studio Code" --wait --reuse-window"#),
        Some("vim"),
    )
    .expect("VISUAL should take precedence");

    let (program, arguments) = parse_editor_command(editor)?;

    assert_eq!(program, "Visual Studio Code");
    assert_eq!(arguments, ["--wait", "--reuse-window"]);
    Ok(())
}

#[test]
fn editor_command_preserves_quoted_windows_paths() -> Result<()> {
    let (program, arguments) =
        parse_editor_command(r#""C:\Program Files\Editor\editor.exe" --wait"#)?;

    assert_eq!(program, r"C:\Program Files\Editor\editor.exe");
    assert_eq!(arguments, ["--wait"]);
    Ok(())
}

#[test]
fn whitespace_only_visual_falls_back_to_editor() {
    assert_eq!(
        configured_editor(Some(" \t "), Some("vim -f")),
        Some("vim -f")
    );
}

#[test]
fn malformed_editor_command_is_rejected() {
    let err = parse_editor_command(r#""unterminated"#)
        .expect_err("an unmatched quote should be rejected");

    assert!(format!("{err:#}").contains("unmatched quote"));
}
