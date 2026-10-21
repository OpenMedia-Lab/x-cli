use assert_cmd::Command;
#[test]
fn native_help_lists_machine_readable_operations() {
    Command::cargo_bin("x")
        .unwrap()
        .args(["native", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("search"))
        .stdout(predicates::str::contains("JSON"));
}
#[test]
fn native_post_rejects_five_images_before_authentication() {
    Command::cargo_bin("x")
        .unwrap()
        .args([
            "native", "post", "test", "--image", "a", "--image", "b", "--image", "c", "--image",
            "d", "--image", "e",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("at most four"));
}

#[test]
fn root_help_displays_native_json_interface() {
    Command::cargo_bin("x")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("native"));
}

#[test]
fn native_parse_errors_are_json() {
    let output = Command::cargo_bin("x")
        .unwrap()
        .args(["native", "read"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("JSON parse error");
    assert_eq!(error["operation"], "parse");
}
