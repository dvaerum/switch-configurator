//! Run commands from stdin on a serial console using the service's own login
//! logic (same prompt handling, factory-default fallback, `--More--` paging as
//! production). Helper for the live lab tests; prints each command's output.
//!
//! Password comes from the environment so it never lands in argv/shell history:
//!   CONSOLE_PASSWORD=... cargo run --example console_exec -- /dev/ttyUSB1 115200 admin < cmds.txt

use std::io::Read;

use anyhow::{bail, Context, Result};
use switch_configurator::ssh::serial::SerialClient;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        bail!("usage: console_exec <device> <baud> <username>   (commands on stdin, CONSOLE_PASSWORD in env)");
    }
    let baud: u32 = args[1].parse().context("baud must be a number")?;
    let password = std::env::var("CONSOLE_PASSWORD").context("CONSOLE_PASSWORD not set")?;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let mut client = SerialClient::new(args[0].clone(), baud);
    client.connect_with_retry(1, 1).await?;
    client.login_with_factory_default_fallback(&args[2], &password).await?;

    for command in input.lines().map(str::trim).filter(|c| !c.is_empty()) {
        println!("### {command}");
        println!("{}", client.execute_command(command).await?);
    }
    client.disconnect().await?;
    Ok(())
}
