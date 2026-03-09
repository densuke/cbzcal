use std::fs;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::str::contains;
use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn write_config(dir: &std::path::Path, fixture_path: &std::path::Path) -> std::path::PathBuf {
    let config_path = dir.join(".cbzcal.yml");
    fs::write(
        &config_path,
        format!(
            r#"
backend: fixture
fixture:
  path: "{}"
"#,
            fixture_path.display()
        ),
    )
    .expect("write config");
    set_private_permissions(&config_path);
    config_path
}

fn write_toml_config(dir: &std::path::Path, fixture_path: &std::path::Path) -> std::path::PathBuf {
    let config_path = dir.join(".cbzcal.toml");
    fs::write(
        &config_path,
        format!(
            r#"
backend = "fixture"

[fixture]
path = "{}"
"#,
            fixture_path.display()
        ),
    )
    .expect("write config");
    set_private_permissions(&config_path);
    config_path
}

#[cfg(unix)]
fn set_private_permissions(path: &std::path::Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod");
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &std::path::Path) {}

#[test]
fn doctor_reports_fixture_backend_as_ready() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .args(["--config", config_path.to_str().expect("path"), "doctor"])
        .assert()
        .success()
        .stdout(contains(r#""ready": true"#));
}

#[test]
fn add_and_list_work_against_fixture_backend() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "add",
            "--json",
            "--title",
            "設計レビュー",
            "--start",
            "2026-03-12T10:00:00+09:00",
            "--end",
            "2026-03-12T11:00:00+09:00",
            "--attendee",
            "alice",
            "--calendar",
            "開発",
        ])
        .assert()
        .success();

    let output = cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "list",
            "--json",
            "--from",
            "2026-03-12T00:00:00+09:00",
            "--to",
            "2026-03-13T00:00:00+09:00",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("json");
    let events = json["data"].as_array().expect("array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["title"], "設計レビュー");
    assert_eq!(json["backend"], "fixture");
}

#[test]
fn event_mutations_render_human_output_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "add",
            "--title",
            "設計レビュー",
            "--date",
            "2026-03-12",
            "--at",
            "10:00",
            "--until",
            "11:00",
        ])
        .assert()
        .success()
        .stdout(contains("追加しました"))
        .stdout(contains("2026-03-12 (Thu)"))
        .stdout(contains("10:00-11:00  設計レビュー"))
        .stdout(contains(" ["))
        .stdout(contains("]"));

    let add_json = cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "add",
            "--json",
            "--title",
            "削除対象",
            "--date",
            "2026-03-13",
            "--at",
            "09:00",
            "--until",
            "10:00",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let add_json: Value = serde_json::from_slice(&add_json).expect("json");
    let id = add_json["data"]["id"].as_str().expect("id").to_string();

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "update",
            "--id",
            &id,
            "--title",
            "更新後",
        ])
        .assert()
        .success()
        .stdout(contains("更新しました"))
        .stdout(contains("更新後"));

    let clone_json = cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "clone",
            "--json",
            "--id",
            &id,
            "--title-suffix",
            " (複製)",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let clone_json: Value = serde_json::from_slice(&clone_json).expect("json");
    let clone_id = clone_json["data"]["id"]
        .as_str()
        .expect("clone id")
        .to_string();

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "delete",
            "--id",
            &clone_id,
        ])
        .assert()
        .success()
        .stdout(contains("削除しました"))
        .stdout(contains("(複製)"));
}

#[test]
fn list_renders_human_readable_output_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "add",
            "--title",
            "設計レビュー",
            "--date",
            "2026-03-12",
            "--at",
            "10:00",
            "--until",
            "11:00",
        ])
        .assert()
        .success();

    cargo_bin_cmd!("cbzcal")
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "events",
            "list",
            "--date",
            "2026-03-12",
        ])
        .assert()
        .success()
        .stdout(contains("2026-03-12 (Thu)"))
        .stdout(contains("10:00-11:00  設計レビュー"))
        .stdout(contains(" ["))
        .stdout(contains("]"));
}

#[test]
fn doctor_discovers_config_in_current_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .current_dir(tempdir.path())
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains(r#""config_path":"#))
        .stdout(contains(".cbzcal.yml"));
}

#[test]
fn doctor_discovers_toml_config_in_current_directory_when_yaml_is_absent() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    write_toml_config(tempdir.path(), &tempdir.path().join("calendar.json"));

    cargo_bin_cmd!("cbzcal")
        .current_dir(tempdir.path())
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains(".cbzcal.toml"));
}

#[cfg(not(windows))]
#[test]
fn doctor_prefers_xdg_config_over_home_config() {
    let root = tempfile::tempdir().expect("tempdir");
    let workdir = root.path().join("work");
    let home_dir = root.path().join("home");
    let xdg_dir = root.path().join("xdg");
    fs::create_dir_all(&workdir).expect("mkdir work");
    fs::create_dir_all(&home_dir).expect("mkdir home");
    fs::create_dir_all(xdg_dir.join("cbzcal")).expect("mkdir xdg");

    let home_config = write_config(&home_dir, &root.path().join("home-calendar.json"));
    let xdg_config = write_config(
        &xdg_dir.join("cbzcal"),
        &root.path().join("xdg-calendar.json"),
    );
    let xdg_config = xdg_config.with_file_name("config.yml");
    fs::rename(xdg_dir.join("cbzcal").join(".cbzcal.yml"), &xdg_config).expect("rename xdg");
    set_private_permissions(&xdg_config);
    set_private_permissions(&home_config);

    let output = cargo_bin_cmd!("cbzcal")
        .current_dir(&workdir)
        .env("HOME", &home_dir)
        .env("XDG_CONFIG_HOME", &xdg_dir)
        .arg("doctor")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: Value = serde_json::from_slice(&output).expect("json");
    assert_eq!(json["config_path"], xdg_config.display().to_string());
}

#[cfg(unix)]
#[test]
fn doctor_rejects_world_readable_config() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let config_path = write_config(tempdir.path(), &tempdir.path().join("calendar.json"));
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644)).expect("chmod 644");

    cargo_bin_cmd!("cbzcal")
        .args(["--config", config_path.to_str().expect("path"), "doctor"])
        .assert()
        .failure()
        .stderr(contains("0400 または 0600"));
}
