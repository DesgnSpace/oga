//! The broker answers as soon as it starts, however long the user's login
//! shell takes to report its PATH.

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use tempfile::TempDir;

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("free port")
        .port()
}

fn health(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200")
}

#[test]
fn health_answers_while_the_login_shell_is_still_starting() {
    let home = TempDir::new().expect("temporary home");
    let release = home.path().join("release-shell");
    let reported = home.path().join("shell-reported");
    let shell = home.path().join("slow-shell");
    // Stands in for an rc file that takes its time: it reports PATH once the
    // test lets it, or after 20 seconds.
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\ni=0\nwhile [ ! -e '{release}' ] && [ $i -lt 400 ]; do sleep 0.05; i=$((i+1)); done\ntouch '{reported}'\nprintf '__OGA_PATH__/opt/login/bin__OGA_END__'\n",
            release = release.display(),
            reported = reported.display(),
        ),
    )
    .expect("shell script");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("shell executable");

    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_oga-cli"))
            .args(["serve", "--port", &port.to_string()])
            .env("HOME", home.path())
            .env("OGA_DB", home.path().join("oga.db"))
            .env("SHELL", &shell)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("broker started"),
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    while !health(port) {
        assert!(
            Instant::now() < deadline,
            "the broker never answered /health"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !reported.exists(),
        "/health answered only after the login shell reported"
    );
    fs::write(&release, "").expect("release the shell");
}
