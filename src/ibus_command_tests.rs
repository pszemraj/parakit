//! Component registration tests with repository-owned fixture directories.

use super::*;
use clap::Parser;

#[derive(Parser, Debug)]
struct TestCli {
    #[command(flatten)]
    ibus: IbusCli,
}

#[test]
fn install_upgrade_and_uninstall_preserve_other_components() {
    let root = crate::test_support::fixture_root("ibus-registration", "lifecycle");
    let other = root.join("other.xml");
    std::fs::write(&other, "unrelated component").unwrap();
    let executable = std::env::current_dir()
        .unwrap()
        .join("target/debug/parakit-ibus");
    let path = install(&root, &executable, "us", "").unwrap();
    let initial = std::fs::read_to_string(&path).unwrap();
    assert!(initial.contains("<layout>us</layout>"));
    assert!(initial.contains("<layout_variant></layout_variant>"));
    assert!(initial.contains("org.freedesktop.IBus.Parakit"));
    assert!(initial.contains("parakit-ibus&apos;</exec>"));

    install(&root, &executable, "de", "nodeadkeys").unwrap();
    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("<layout>de</layout>"));
    assert!(updated.contains("<layout_variant>nodeadkeys</layout_variant>"));
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        "unrelated component"
    );

    assert!(uninstall(&root).unwrap());
    assert!(!path.exists());
    assert!(!uninstall(&root).unwrap());
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        "unrelated component"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn executable_command_keeps_special_characters_in_one_argument() {
    let executable = "/home/user/Parakit's & <tools>/parakit-ibus";
    let quoted = quote_argument(executable);
    let words = shlex::split(&quoted).unwrap();
    assert_eq!(words, [executable]);
    let xml = component_xml(Path::new(executable), "us&intl", "<variant>").unwrap();
    assert!(xml.contains("Parakit&apos;\\&apos;&apos;s &amp; &lt;tools&gt;"));
    assert!(xml.contains("<layout>us&amp;intl</layout>"));
    assert!(xml.contains("<layout_variant>&lt;variant&gt;</layout_variant>"));
    assert!(!xml.contains("<tools>"));
}

#[test]
fn registration_rejects_relative_executable() {
    let error = component_xml(Path::new("target/debug/parakit"), "us", "").unwrap_err();
    assert!(error.to_string().contains("absolute path"));
}

#[test]
fn install_cli_defaults_to_us_and_accepts_layout_variant() {
    let cli = TestCli::parse_from(["ibus", "install"]);
    let Some(IbusCommand::Install {
        layout, variant, ..
    }) = cli.ibus.command
    else {
        panic!("expected install command");
    };
    assert_eq!(layout, "us");
    assert!(variant.is_empty());

    let cli = TestCli::parse_from([
        "ibus",
        "install",
        "--component-dir",
        "target/tmp/ibus-component",
        "--layout",
        "de",
        "--variant",
        "nodeadkeys",
    ]);
    let Some(IbusCommand::Install {
        component_dir,
        layout,
        variant,
    }) = cli.ibus.command
    else {
        panic!("expected install command");
    };
    assert_eq!(component_dir, Path::new("target/tmp/ibus-component"));
    assert_eq!(layout, "de");
    assert_eq!(variant, "nodeadkeys");
}
