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

pub mod generate;

use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use structopt::StructOpt;

pub static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

#[derive(StructOpt)]
pub struct Cli {
    #[structopt(
        short,
        long,
        default_value = "info",
        possible_values = &["trace", "debug", "info", "warn", "error"]
    )]
    pub log_level: tracing::Level,

    #[structopt(subcommand)]
    pub commands: Commands,
}

#[derive(StructOpt)]
pub enum Commands {
    Generate(GenerateArgs),
}

#[derive(StructOpt)]
pub struct GenerateArgs {
    /// OpenAPIv3 Spec file to generate the SDK from
    #[structopt(short = "s", long = "schema")]
    pub schema: PathBuf,

    /// The output directory where the generated bindings will be placed
    #[structopt(short = "o", long = "output-dir")]
    pub output: PathBuf,

    /// Skygen config for generating the SDK.
    #[structopt(short = "c", long = "config")]
    pub config: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub crate_name: String,
    pub version: String,
    pub edition: Option<String>,
    pub description: String,
    pub lib_status: String,
    pub keywords: Vec<String>,
    pub api_url: String,
    pub authors: Vec<String>,
    pub include_only: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}
