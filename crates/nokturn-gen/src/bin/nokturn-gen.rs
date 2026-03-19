// Copyright 2026 Cloudflavor GmbH

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at

// http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Context;
use nokturn_gen::{Cli, Commands, Config, generate::generate_sdk};
use structopt::StructOpt;
use tokio::fs;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::Subscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Cli::from_args();

    let opts_level = opts.log_level;
    let env_filter = EnvFilter::new(opts_level.as_str());

    let subscriber = Subscriber::builder()
        .with_ansi(true)
        .with_env_filter(env_filter)
        .with_writer(std::io::stdout)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    match opts.commands {
        Commands::Generate(args) => {
            let ext = args
                .schema
                .extension()
                .and_then(|s| s.to_str())
                .with_context(|| "failed to parse schema file extension")?;

            let c = fs::read(&args.config)
                .await
                .with_context(|| "failed to read schema file")?;
            let _config: Config = toml::from_slice(c.as_slice())
                .with_context(|| "failed to deserialize schema toml config")?;
            let d = fs::read_to_string(&args.schema)
                .await
                .with_context(|| "failed to read openapi schema")?;
            generate_sdk(&_config, d.as_str(), ext, &args.output)
                .await
                .with_context(|| "failed to generate sdk")?;
        }
    }

    Ok(())
}
