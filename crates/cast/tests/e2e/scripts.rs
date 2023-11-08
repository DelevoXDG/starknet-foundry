use crate::helpers::constants::{SCRIPTS_DIR, URL};
use snapbox::cmd::{cargo_bin, Command};
#[tokio::test]
async fn test_happy_case() {
    let script_path = "hello_world.cairo";
    let args = vec![
        "--accounts-file",
        "../../accounts/accounts.json",
        "--account",
        "user1",
        "--url",
        URL,
        "script",
        &script_path,
    ];

    let snapbox = Command::new(cargo_bin!("sncast"))
        .current_dir(SCRIPTS_DIR.to_owned() + "/hello_world")
        .args(args);
    let _output =
        String::from_utf8(snapbox.assert().success().get_output().stdout.clone()).unwrap();
}