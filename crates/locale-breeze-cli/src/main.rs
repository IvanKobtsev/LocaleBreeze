use anyhow::{Result, bail};
use std::path::PathBuf;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next().as_deref()) {
        (Some("lsp"), Some("--stdio")) | (Some("--stdio"), None) => {
            let mut config = None;
            while let Some(arg) = args.next() {
                if arg == "--config" {
                    config = args.next().map(PathBuf::from);
                } else {
                    bail!("unknown argument: {arg}");
                }
            }
            locale_breeze_lsp::run_stdio(config)
        }
        _ => bail!("usage: locale-breeze lsp --stdio [--config <path>]"),
    }
}
