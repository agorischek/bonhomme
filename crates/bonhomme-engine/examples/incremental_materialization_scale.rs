use anyhow::{Context, Result};
use bonhomme_engine::run_materialization_scale_benchmark;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let operation_count = parse_count(args.next(), 1_000_000, "operation count")?;
    let symbol_count = parse_count(args.next(), operation_count, "symbol count")?;
    if args.next().is_some() {
        anyhow::bail!("usage: incremental_materialization_scale [operation-count] [symbol-count]");
    }

    let report = run_materialization_scale_benchmark(operation_count, symbol_count)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_count(value: Option<String>, default: usize, label: &str) -> Result<usize> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid {label}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}
