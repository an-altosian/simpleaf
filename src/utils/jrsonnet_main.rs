// This crate is a modified version of jrsonnet cli.
// https://github.com/CertainLach/jrsonnet/blob/master/cmds/jrsonnet/src/main.rs

use anyhow::anyhow;
use clap::Parser;
use jrsonnet_cli::{ConfigureState, GeneralOpts, ManifestOpts, OutputOpts, TraceOpts};
use jrsonnet_evaluator::{
    apply_tla,
    error::{Error as JrError, ErrorKind},
    State,
};
use std::path::{Path, PathBuf};

use super::workflow_utils::ProtocolEstuary;

#[derive(Parser)]
#[command(next_help_heading = "DEBUG")]
struct DebugOpts {
    /// Required OS stack size.
    /// This shouldn't be changed unless jrsonnet is failing with stack overflow error.
    #[arg(long, id = "size")]
    pub os_stack: Option<usize>,
}

#[derive(Parser)]
#[command(next_help_heading = "INPUT")]
struct InputOpts {
    /// Treat input as code, evaluate them instead of reading file
    #[arg(long, short = 'e')]
    pub exec: bool,

    /// Path to the file to be compiled if `--evaluate` is unset, otherwise code itself
    pub input: Option<String>,
}

/// Jsonnet commandline interpreter (Rust implementation)
#[derive(Parser)]
#[command(
    args_conflicts_with_subcommands = true,
    disable_version_flag = true,
    version,
    author
)]
struct Opts {
    #[clap(flatten)]
    input: InputOpts,
    #[clap(flatten)]
    general: GeneralOpts,

    #[clap(flatten)]
    trace: TraceOpts,
    #[clap(flatten)]
    manifest: ManifestOpts,
    #[clap(flatten)]
    output: OutputOpts,
    #[clap(flatten)]
    debug: DebugOpts,
}

pub fn parse_jsonnet(
    config_file_path: &Path,
    output: &Path,
    protocol_estuary: ProtocolEstuary,
    lib_paths: &Option<Vec<PathBuf>>,
) -> anyhow::Result<String> {
    // define jrsonnet argumetns
    // config file
    let input_config_file_path = config_file_path
        .to_str()
        .expect("Could not convert workflow config file path to str");
    let jpath_config_file_path = config_file_path
        .parent()
        .expect("Could not get the parent dir of the config file.")
        .to_str()
        .expect("Could not convert the parent dir of the config file to str.");
    // external code
    let ext_output = format!(r#"output='{}'"#, output.display());
    let ext_utils_file_path = r#"utils=import 'simpleaf_workflow_utils.libsonnet'"#;

    // af_home_dir
    let jpath_pe_utils = protocol_estuary
        .utils_dir
        .to_str()
        .expect("Could not convert Protocol Estuarys path to str");

    // create command vector for clap parser
    let mut jrsonnet_cmd_vec = vec![
        "jrsonnet",
        input_config_file_path,
        "--ext-code",
        &ext_output,
        "--ext-code",
        ext_utils_file_path,
        "--jpath",
        jpath_pe_utils,
        "--jpath",
        jpath_config_file_path,
    ];

    // if the user provides more lib search path, then assign it.
    if let Some(lib_paths) = lib_paths {
        for lib_path in lib_paths {
            jrsonnet_cmd_vec.push("--jpath");
            jrsonnet_cmd_vec.push(lib_path.to_str().expect("Could not convert path to "));
        }
    }

    let opts: Opts = Opts::parse_from(jrsonnet_cmd_vec);
    main_catch(opts)
}

#[derive(thiserror::Error, Debug)]
enum Error {
    // Handled differently
    #[error("evaluation error")]
    Evaluation(JrError),
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("input is not utf8 encoded")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("missing input argument")]
    MissingInputArgument,
    #[error("Evaluated empty JSON record")]
    EmptyJSON,
}
impl From<JrError> for Error {
    fn from(e: JrError) -> Self {
        Self::Evaluation(e)
    }
}
impl From<ErrorKind> for Error {
    fn from(e: ErrorKind) -> Self {
        Self::from(JrError::from(e))
    }
}

fn main_catch(opts: Opts) -> anyhow::Result<String> {
    let s = State::default();
    let trace = opts
        .trace
        .configure(&s)
        .expect("this configurator doesn't fail");
    match main_real(&s, opts) {
        Ok(js) => Ok(js),
        Err(e) => {
            if let Error::Evaluation(e) = e {
                let mut out = String::new();
                trace.write_trace(&mut out, &e).expect("format error");
                Err(anyhow!(
                    "Error Occurred when evaluating a configuration file. Cannot proceed. {out}"
                ))
            } else {
                Err(anyhow!(
                    "Found invalid configuration file. The error message was: {e}"
                ))
            }
        }
    }
}

fn main_real(s: &State, opts: Opts) -> Result<String, Error> {
    let (tla, _gc_guard) = opts.general.configure(s)?;
    let manifest_format = opts.manifest.configure(s)?;

    let input = opts.input.input.ok_or(Error::MissingInputArgument)?;
    let val = s
        .import(input)
        .expect("Cannot import workflow config file.");

    let val = apply_tla(s.clone(), &tla, val)?;

    let output = val.manifest(manifest_format)?;
    if !output.is_empty() {
        Ok(output)
    } else {
        Err(Error::EmptyJSON)
    }
}
