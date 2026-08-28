//! A stand-in Folia server that speaks RCON, for exercising mctop without a
//! real one. Run it, then point mctop at the address it prints:
//!
//! ```text
//! cargo run --example mock_server                 # in one terminal
//! MCTOP_RCON_PASSWORD=secret cargo run -- --address 127.0.0.1:25575
//! ```
//!
//! Pass `--paper` for a server with no region report, `--broken` for one whose
//! output no parser recognises, `--hung` for one that authenticates and then
//! stops answering, and `--port N` to move it.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// RCON packet types, as Valve numbered them.
const AUTH: i32 = 3;
const AUTH_RESPONSE: i32 = 2;
const EXEC_COMMAND: i32 = 2;
const RESPONSE_VALUE: i32 = 0;

/// Which flavour of server to imitate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// Reports per-region detail, as Folia does.
    Folia,
    /// Global figures only, as Paper does.
    Paper,
    /// Answers everything with something no parser can read.
    Unrecognisable,
    /// Authenticates, then never answers a command — a server whose main thread
    /// has stopped ticking, which is exactly when an operator is watching.
    Unresponsive,
}

#[allow(dead_code)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut flavour = Flavour::Folia;
    let mut port = 25575u16;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--paper" => flavour = Flavour::Paper,
            "--broken" => flavour = Flavour::Unrecognisable,
            "--hung" => flavour = Flavour::Unresponsive,
            "--port" => {
                port = arguments
                    .next()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(port)
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("mock {flavour:?} server on {}", listener.local_addr()?);
    println!("password: secret");
    serve(listener, "secret".into(), flavour).await;
    Ok(())
}

/// Accept connections until the listener is dropped.
pub async fn serve(listener: TcpListener, password: String, flavour: Flavour) {
    serve_counting(listener, password, flavour, Arc::new(AtomicUsize::new(0))).await;
}

/// As [`serve`], but counting every command executed, so a test can tell how
/// many round trips mctop actually made.
pub async fn serve_counting(
    listener: TcpListener,
    password: String,
    flavour: Flavour,
    commands: Arc<AtomicUsize>,
) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let password = password.clone();
        let commands = Arc::clone(&commands);
        tokio::spawn(async move {
            let _ = session(stream, password, flavour, commands).await;
        });
    }
}

async fn session(
    mut stream: TcpStream,
    password: String,
    flavour: Flavour,
    commands: Arc<AtomicUsize>,
) -> io::Result<()> {
    let mut authenticated = false;

    loop {
        let (id, kind, body) = match read_packet(&mut stream).await {
            Ok(packet) => packet,
            // A closed connection is how every session ends.
            Err(_) => return Ok(()),
        };

        match kind {
            AUTH => {
                authenticated = body == password;
                // A real server sends an empty value packet before the verdict.
                write_packet(&mut stream, id, RESPONSE_VALUE, "").await?;
                let id = if authenticated { id } else { -1 };
                write_packet(&mut stream, id, AUTH_RESPONSE, "").await?;
                if !authenticated {
                    return Ok(());
                }
            }
            EXEC_COMMAND if authenticated && flavour == Flavour::Unresponsive => {
                // Leave the connection open and say nothing.
            }
            EXEC_COMMAND if authenticated => {
                // The client sends an empty packet to mark the end of a
                // response; that is not a command anyone asked for.
                if !body.trim().is_empty() {
                    commands.fetch_add(1, Ordering::SeqCst);
                }
                let response = respond(&body, flavour);
                write_packet(&mut stream, id, RESPONSE_VALUE, &response).await?;
            }
            _ => return Ok(()),
        }
    }
}

fn respond(command: &str, flavour: Flavour) -> String {
    if flavour == Flavour::Unrecognisable {
        return "Unknown or incomplete command, see below for error".into();
    }

    match command.split_whitespace().next().unwrap_or_default() {
        "tps" => match flavour {
            Flavour::Folia => folia_tps(),
            _ => "\u{a7}6TPS from last 1m, 5m, 15m: \u{a7}a*20.0\u{a7}6, \u{a7}a19.87\u{a7}6, \u{a7}a19.94"
                .into(),
        },
        "mspt" => "\u{a7}6Server tick times \u{a7}e(\u{a7}aavg\u{a7}e/\u{a7}amin\u{a7}e/\u{a7}amax\u{a7}e)\u{a7}6 from last 5s, 10s, 1m:\n\
             \u{a7}6\u{25f4} \u{a7}a4.21\u{a7}e/\u{a7}a0.94\u{a7}e/\u{a7}a18.70\u{a7}6, \u{a7}a4.88/1.02/31.40\u{a7}6, \u{a7}a5.10/0.91/47.20"
            .into(),
        "list" => {
            "There are 3 of a max of 60 players online: alice, bob, carol".into()
        }
        "version" => match flavour {
            Flavour::Folia => "This server is running Folia version 1.21.4-DEV-a1b2c3d (MC: 1.21.4) (Implementing API version 1.21.4-R0.1-SNAPSHOT)".into(),
            _ => "This server is running Paper version 1.21.4-64 (MC: 1.21.4)".into(),
        },
        // An empty body is the end-of-response marker the client sends.
        "" => String::new(),
        other => format!("Unknown or incomplete command, see below for error\n{other}<--[HERE]"),
    }
}

fn folia_tps() -> String {
    let mut report = String::from(
        "\u{a7}6TPS from last 5s, 15s, 1m, 5m, 15m: \u{a7}a19.87, 19.91, 19.70, 19.90, 20.00\n\
         \u{a7}6Total Regions: \u{a7}a4\n\
         \u{a7}6Region Threads: \u{a7}a6\n",
    );

    for (world, x, z, tps, mspt, util, players, entities, chunks) in [
        ("world", -12, 44, 19.98, 3.21, 6.4, 2, 812, 441),
        ("world", 96, -8, 14.20, 48.60, 97.2, 1, 4102, 1204),
        ("world_nether", 30, 8, 20.00, 1.10, 2.2, 0, 96, 121),
        ("world_the_end", -4, 2, 19.94, 4.80, 9.6, 0, 214, 289),
    ] {
        report.push_str(&format!(
            "\u{a7}6Region \u{a7}f{world} \u{a7}7({x}, {z})\u{a7}6: \
             tps=\u{a7}a{tps:.2}\u{a7}6, mspt=\u{a7}a{mspt:.2}\u{a7}6, util=\u{a7}a{util:.1}%\u{a7}6, \
             players=\u{a7}a{players}\u{a7}6, entities=\u{a7}a{entities}\u{a7}6, chunks=\u{a7}a{chunks}\n"
        ));
    }

    report
}

async fn read_packet(stream: &mut TcpStream) -> io::Result<(i32, i32, String)> {
    let length = stream.read_i32_le().await?;
    if !(10..=4096).contains(&length) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad length"));
    }

    let id = stream.read_i32_le().await?;
    let kind = stream.read_i32_le().await?;

    let mut body = vec![0u8; length as usize - 8];
    stream.read_exact(&mut body).await?;
    // The body carries two trailing nulls: its own terminator and the packet's.
    while body.last() == Some(&0) {
        body.pop();
    }

    Ok((id, kind, String::from_utf8_lossy(&body).into_owned()))
}

async fn write_packet(stream: &mut TcpStream, id: i32, kind: i32, body: &str) -> io::Result<()> {
    let mut packet = Vec::with_capacity(body.len() + 14);
    packet.extend_from_slice(&(body.len() as i32 + 10).to_le_bytes());
    packet.extend_from_slice(&id.to_le_bytes());
    packet.extend_from_slice(&kind.to_le_bytes());
    packet.extend_from_slice(body.as_bytes());
    packet.extend_from_slice(&[0, 0]);
    stream.write_all(&packet).await
}
