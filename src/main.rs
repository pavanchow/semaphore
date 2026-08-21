use clap::{Parser, Subcommand};
use semaphore::Client;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "semaphore", about = "A length-prefixed binary message protocol over TCP")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server, holding an in-memory key-value store.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
    },
    /// Send PING and expect PONG.
    Ping {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
    },
    /// Set a key to a value.
    Set {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
        key: String,
        value: String,
    },
    /// Get a key's value.
    Get {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
        key: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve { addr } => {
            println!("semaphore server listening on {addr}");
            semaphore::serve(&addr).map_err(|e| e.to_string())
        }
        Command::Ping { addr } => Client::connect(&addr)
            .map_err(|e| e.to_string())
            .and_then(|mut c| c.ping().map_err(|e| e.to_string()))
            .map(|_| println!("PONG")),
        Command::Set { addr, key, value } => Client::connect(&addr)
            .map_err(|e| e.to_string())
            .and_then(|mut c| c.set(&key, value.as_bytes()).map_err(|e| e.to_string()))
            .map(|_| println!("OK")),
        Command::Get { addr, key } => Client::connect(&addr)
            .map_err(|e| e.to_string())
            .and_then(|mut c| c.get(&key).map_err(|e| e.to_string()))
            .map(|v| match v {
                Some(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
                None => println!("(not found)"),
            }),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
