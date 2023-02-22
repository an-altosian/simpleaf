use tracing::{info, warn};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use anyhow::{bail, Context};
use clap::{builder::ArgPredicate, ArgGroup, Parser, Subcommand};
use cmd_lib::run_fun;
use serde_json::json;

use time::{Duration, Instant};

use std::io::BufReader;
use std::io::Write;
// use std::io::{Seek, SeekFrom};
use std::io::{Seek};
use std::path::{PathBuf,Path};
use std::{env, fs};

mod utils;
use utils::af_utils::*;
use utils::prog_utils::*;
use utils::workflow_utils::*;

use crate::utils::prog_utils;

#[derive(Clone, Debug)]
pub enum ReferenceType {
    SplicedIntronic,
    SplicedUnspliced,
}

fn ref_type_parser(s: &str) -> Result<ReferenceType, String> {
    match s {
        "spliced+intronic" | "splici" => Ok(ReferenceType::SplicedIntronic),
        "spliced+unspliced" | "spliceu" => Ok(ReferenceType::SplicedUnspliced),
        t => Err(format!("Do not recognize reference type {}", t)),
    }
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// build the (expanded) reference index
    #[command(arg_required_else_help = true)]
    #[command(group(
             ArgGroup::new("reftype")
             .required(true)
             .args(["fasta", "ref_seq"])
    ))]
    Index {
        /// specify whether an expanded reference, spliced+intronic (or splici) or spliced+unspliced (or spliceu), should be built
        #[arg(long, help_heading="Expanded Reference Options", display_order = 1, default_value = "spliced+intronic", value_parser = ref_type_parser)]
        ref_type: ReferenceType,

        /// reference genome to be used for the expanded reference construction
        #[arg(short, long, help_heading="Expanded Reference Options", display_order = 2, 
              requires_ifs([
                (ArgPredicate::IsPresent, "gtf") 
              ]),
              conflicts_with = "ref_seq")]
        fasta: Option<PathBuf>,

        /// reference GTF file to be used for the expanded reference construction
        #[arg(
            short,
            long,
            help_heading = "Expanded Reference Options",
            display_order = 3,
            requires = "fasta",
            conflicts_with = "ref_seq"
        )]
        gtf: Option<PathBuf>,

        /// the target read length the splici index will be built for
        #[arg(
            short,
            long,
            help_heading = "Expanded Reference Options",
            display_order = 4,
            requires = "fasta",
            conflicts_with = "ref_seq"
        )]
        rlen: Option<u32>,

        /// deduplicate identical sequences in pyroe when building an expanded reference  reference
        #[arg(
            long = "dedup",
            help_heading = "Expanded Reference Options",
            display_order = 5,
            requires = "fasta",
            conflicts_with = "ref_seq"
        )]
        dedup: bool,

        /// target sequences (provide target sequences directly; avoid expanded reference construction)
        #[arg(long, alias = "refseq", help_heading = "Direct Reference Options", display_order = 6,
              conflicts_with_all = ["dedup", "unspliced", "spliced", "rlen", "gtf", "fasta"])]
        ref_seq: Option<PathBuf>,

        /// path to FASTA file with extra spliced sequence to add to the index
        #[arg(
            long,
            help_heading = "Expanded Reference Options",
            display_order = 7,
            requires = "fasta",
            conflicts_with = "ref_seq"
        )]
        spliced: Option<PathBuf>,

        /// path to FASTA file with extra unspliced sequence to add to the index
        #[arg(
            long,
            help_heading = "Expanded Reference Options",
            display_order = 8,
            requires = "fasta",
            conflicts_with = "ref_seq"
        )]
        unspliced: Option<PathBuf>,

        /// use piscem instead of salmon for indexing and mapping
        #[arg(long, help_heading = "Piscem Index Options", display_order = 1)]
        use_piscem: bool,

        /// the value of m to be used to construct the piscem index (must be < k)
        #[arg(
            short = 'm',
            long = "minimizer-length",
            default_value_t = 19,
            requires = "use_piscem",
            help_heading = "Piscem Index Options",
            display_order = 2
        )]
        minimizer_length: u32,

        /// path to output directory (will be created if it doesn't exist)
        #[arg(short, long, display_order = 1)]
        output: PathBuf,

        /// overwrite existing files if the output directory is already populated
        #[arg(long, display_order = 6)]
        overwrite: bool,

        /// number of threads to use when running
        #[arg(short, long, default_value_t = 16, display_order = 2)]
        threads: u32,

        /// the value of k to be used to construct the index
        #[arg(
            short = 'k',
            long = "kmer-length",
            default_value_t = 31,
            display_order = 3
        )]
        kmer_length: u32,

        /// keep duplicated identical sequences when constructing the index
        #[arg(long, display_order = 4)]
        keep_duplicates: bool,

        /// if this flag is passed, build the sparse rather than dense index for mapping
        #[arg(
            short = 'p',
            long = "sparse",
            conflicts_with = "use_piscem",
            display_order = 5
        )]
        sparse: bool,
    },
    /// add a new custom chemistry to geometry mapping
    #[command(arg_required_else_help = true)]
    AddChemistry {
        /// the name to give the chemistry
        #[arg(short, long)]
        name: String,
        /// the geometry to which the chemistry maps
        #[arg(short, long)]
        geometry: String,
    },
    /// inspect the current configuration
    Inspect {},
    /// quantify a sample
    #[command(arg_required_else_help = true)]
    #[command(group(
            ArgGroup::new("filter")
            .required(true)
            .args(["knee", "unfiltered_pl", "forced_cells", "expect_cells"])
            ))]
    #[command(group(
            ArgGroup::new("input-type")
            .required(true)
            .args(["index", "map_dir"])
            ))]
    Quant {
        /// chemistry
        #[arg(short, long)]
        chemistry: String,

        /// output directory
        #[arg(short, long)]
        output: PathBuf,

        /// number of threads to use when running
        #[arg(short, long, default_value_t = 16)]
        threads: u32,

        /// path to index
        #[arg(
            short = 'i',
            long = "index",
            help_heading = "Mapping Options",
            requires_ifs([
                (ArgPredicate::IsPresent, "reads1"),
                (ArgPredicate::IsPresent, "reads2")
            ])
        )]
        index: Option<PathBuf>,

        /// comma-separated list of paths to read 1 files
        #[arg(
            short = '1',
            long = "reads1",
            help_heading = "Mapping Options",
            value_delimiter = ',',
            requires = "index",
            conflicts_with = "map_dir"
        )]
        reads1: Option<Vec<PathBuf>>,

        /// comma-separated list of paths to read 2 files
        #[arg(
            short = '2',
            long = "reads2",
            help_heading = "Mapping Options",
            value_delimiter = ',',
            requires = "index",
            conflicts_with = "map_dir"
        )]
        reads2: Option<Vec<PathBuf>>,

        /// use selective-alignment for mapping (instead of pseudoalignment with structural
        /// constraints).
        #[arg(short = 's', long, help_heading = "Mapping Options")]
        use_selective_alignment: bool,

        /// use piscem for mapping (requires that index points to the piscem index)
        #[arg(long, requires = "index", help_heading = "Mapping Options")]
        use_piscem: bool,

        /// path to a mapped output directory containing a RAD file to skip mapping
        #[arg(long = "map-dir", conflicts_with_all = ["index", "reads1", "reads2"], help_heading = "Mapping Options")]
        map_dir: Option<PathBuf>,

        /// use knee filtering mode
        #[arg(short, long, help_heading = "Permit List Generation Options")]
        knee: bool,

        /// use unfiltered permit list
        #[arg(short, long, help_heading = "Permit List Generation Options")]
        unfiltered_pl: Option<Option<PathBuf>>,

        /// use forced number of cells
        #[arg(short, long, help_heading = "Permit List Generation Options")]
        forced_cells: Option<usize>,

        /// use a filtered, explicit permit list
        #[arg(short = 'x', long, help_heading = "Permit List Generation Options")]
        explicit_pl: Option<PathBuf>,

        /// use expected number of cells
        #[arg(short, long, help_heading = "Permit List Generation Options")]
        expect_cells: Option<usize>,

        /// The expected direction/orientation of alignments in the chemistry being processed. If
        /// not provided, will default to `fw` for 10xv2/10xv3, otherwise `both`.
        #[arg(short = 'd', long, help_heading="Permit List Generation Options", value_parser = clap::builder::PossibleValuesParser::new(["fw", "rc", "both"]))]
        expected_ori: Option<String>,

        /// minimum read count threshold for a cell to be retained/processed; only used with --unfiltered-pl
        #[arg(
            long,
            help_heading = "Permit List Generation Options",
            default_value_t = 10
        )]
        min_reads: usize,

        /// transcript to gene map
        #[arg(short = 'm', long, help_heading = "UMI Resolution Options")]
        t2g_map: Option<PathBuf>,

        /// resolution mode
        #[arg(short, long, help_heading = "UMI Resolution Options", value_parser = clap::builder::PossibleValuesParser::new(["cr-like", "cr-like-em", "parsimony", "parsimony-em", "parsimony-gene", "parsimony-gene-em"]))]
        resolution: String,
    },
    /// set paths to the programs that simpleaf will use
    SetPaths {
        /// path to salmon to use
        #[arg(short, long)]
        salmon: Option<PathBuf>,
        /// path to piscem to use
        #[arg(short, long)]
        piscem: Option<PathBuf>,
        /// path to alein-fry to use
        #[arg(short, long)]
        alevin_fry: Option<PathBuf>,
        /// path to pyroe to use
        #[arg(short = 'r', long)]
        pyroe: Option<PathBuf>,
    },

    /// run workflow according to a JSON file
    RunWorkflow {
        /// comma-separated list of paths to read 1 files
        #[arg(short, long, value_delimiter = ',')]
        jsons: Vec<PathBuf>,
    },
}

/// simplifying alevin-fry workflows
#[derive(Debug, Parser)]
#[command(author, version, about)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}


fn set_paths(af_home_path: PathBuf, set_path_args: Commands) -> anyhow::Result<()> {
    const AF_HOME: &str = "ALEVIN_FRY_HOME";
    match set_path_args {
        Commands::SetPaths {
            salmon,
            piscem,
            alevin_fry,
            pyroe,
        } => {
            // create AF_HOME if needed
            if !af_home_path.as_path().is_dir() {
                info!(
                    "The {} directory, {}, doesn't exist, creating...",
                    AF_HOME,
                    af_home_path.display()
                );
                fs::create_dir_all(af_home_path.as_path())?;
            }

            let rp = get_required_progs_from_paths(salmon, piscem, alevin_fry, pyroe)?;

            let have_mapper = rp.salmon.is_some() || rp.piscem.is_some();
            if !have_mapper {
                bail!("Suitable executable for piscem or salmon not found — at least one of these must be available.");
            }
            if rp.alevin_fry.is_none() {
                bail!("Suitable alevin_fry executable not found.");
            }
            if rp.pyroe.is_none() {
                bail!("Suitable pyroe executable not found.");
            }

            let simpleaf_info_file = af_home_path.join("simpleaf_info.json");
            let simpleaf_info = json!({ "prog_info": rp });

            std::fs::write(
                &simpleaf_info_file,
                serde_json::to_string_pretty(&simpleaf_info).unwrap(),
            )
            .with_context(|| format!("could not write {}", simpleaf_info_file.display()))?;
        }
        _ => {
            bail!("unexpected command")
        }
    }
    Ok(())
}

fn build_ref_and_index(af_home_path: &Path, index_args: Commands) -> anyhow::Result<()> {
    match index_args {
        // if we are building the reference and indexing
        Commands::Index {
            ref_type,
            fasta,
            gtf,
            rlen,
            spliced,
            unspliced,
            dedup,
            keep_duplicates,
            ref_seq,
            output,
            use_piscem,
            kmer_length,
            minimizer_length,
            overwrite,
            sparse,
            mut threads,
        } => {
            let v: serde_json::Value = inspect_af_home(af_home_path)?;
            // Read the JSON contents of the file as an instance of `User`.
            let rp: ReqProgs = serde_json::from_value(v["prog_info"].clone())?;

            // we are building a custom reference
            if fasta.is_some() {
                // make sure that the spliced+unspliced reference
                // is supported if that's what's being requested.
                match ref_type {
                    ReferenceType::SplicedUnspliced => {
                        let v = rp.pyroe.clone().unwrap().version;
                        if let Err(e) =
                            prog_utils::check_version_constraints("pyroe", ">=0.8.1, <1.0.0", &v)
                        {
                            bail!(e);
                        }
                    }
                    ReferenceType::SplicedIntronic => {
                        // in this branch we are making a spliced+intronic (splici) index, so
                        // the user must have specified the read length.
                        if rlen.is_none() {
                            bail!(format!("A spliced+intronic reference was requested, but no read length argument (--rlen) was provided."));
                        }
                    }
                }
            }

            let info_file = output.join("index_info.json");
            let mut index_info = json!({
                "command" : "index",
                "version_info" : rp,
                "args" : {
                    "output" : output,
                    "overwrite" : overwrite,
                    "keep_duplicates" : keep_duplicates,
                    "sparse" : sparse,
                    "threads" : threads,
                }
            });

            run_fun!(mkdir -p $output)?;

            // wow, the compiler is smart enough to
            // figure out that this one need not be
            // mutable because it is set once in either
            // branch of the conditional below.
            let reference_sequence;
            // these may or may not be set, so must be
            // mutable.
            let mut splici_t2g = None;
            let mut pyroe_duration = None;
            let pyroe_cmd_string: String;

            // if we are generating a splici reference
            if let (Some(fasta), Some(gtf)) = (fasta, gtf) {
                let mut input_files = vec![fasta.clone(), gtf.clone()];

                let outref = output.join("ref");
                run_fun!(mkdir -p $outref)?;

                let read_len;
                let ref_file;
                let t2g_file;

                match ref_type {
                    ReferenceType::SplicedIntronic => {
                        read_len = rlen.unwrap();
                        ref_file = format!("splici_fl{}.fa", read_len - 5);
                        t2g_file = outref.join(format!("splici_fl{}_t2g_3col.tsv", read_len - 5));
                    }
                    ReferenceType::SplicedUnspliced => {
                        read_len = 0;
                        ref_file = String::from("spliceu.fa");
                        t2g_file = outref.join("spliceu_t2g_3col.tsv");
                    }
                }

                index_info["t2g_file"] = json!(&t2g_file);
                index_info["args"]["fasta"] = json!(&fasta);
                index_info["args"]["gtf"] = json!(&gtf);
                index_info["args"]["spliced"] = json!(&spliced);
                index_info["args"]["unspliced"] = json!(&unspliced);
                index_info["args"]["dedup"] = json!(dedup);

                std::fs::write(
                    &info_file,
                    serde_json::to_string_pretty(&index_info).unwrap(),
                )
                .with_context(|| format!("could not write {}", info_file.display()))?;

                // set the splici_t2g option
                splici_t2g = Some(t2g_file);

                let mut pyroe_cmd =
                    std::process::Command::new(format!("{}", rp.pyroe.unwrap().exe_path.display()));
                // select the command to run
                match ref_type {
                    ReferenceType::SplicedIntronic => {
                        pyroe_cmd.arg("make-splici");
                    }
                    ReferenceType::SplicedUnspliced => {
                        pyroe_cmd.arg("make-spliceu");
                    }
                };

                // if the user wants to dedup output sequences
                if dedup {
                    pyroe_cmd.arg(String::from("--dedup-seqs"));
                }

                // extra spliced sequence
                if let Some(es) = spliced {
                    pyroe_cmd.arg(String::from("--extra-spliced"));
                    pyroe_cmd.arg(format!("{}", es.display()));
                    input_files.push(es);
                }

                // extra unspliced sequence
                if let Some(eu) = unspliced {
                    pyroe_cmd.arg(String::from("--extra-unspliced"));
                    pyroe_cmd.arg(format!("{}", eu.display()));
                    input_files.push(eu);
                }

                pyroe_cmd.arg(fasta).arg(gtf);

                // if making splici the second positional argument is the
                // read length.
                if let ReferenceType::SplicedIntronic = ref_type {
                    pyroe_cmd.arg(format!("{}", read_len));
                };

                // the output directory
                pyroe_cmd.arg(&outref);

                check_files_exist(&input_files)?;

                // print pyroe command
                pyroe_cmd_string = get_cmd_line_string(&pyroe_cmd);
                info!("pyroe cmd : {}", pyroe_cmd_string);

                let pyroe_start = Instant::now();
                let cres =
                    prog_utils::execute_command(&mut pyroe_cmd, CommandVerbosityLevel::Verbose)
                        .expect(
                            "could not execute pyroe (for generating reference transcriptome).",
                        );
                pyroe_duration = Some(pyroe_start.elapsed());

                if !cres.status.success() {
                    bail!("pyroe failed to return succesfully {:?}", cres.status);
                }

                reference_sequence = Some(outref.join(ref_file));
            } else {
                // we are running on a set of references directly

                // in this path (due to the argument parser requiring
                // either --fasta or --ref-seq, ref-seq should be safe to
                // unwrap).
                index_info["args"]["ref-seq"] = json!(ref_seq.clone().unwrap());

                std::fs::write(
                    &info_file,
                    serde_json::to_string_pretty(&index_info).unwrap(),
                )
                .with_context(|| format!("could not write {}", info_file.display()))?;

                pyroe_cmd_string = String::from("");
                reference_sequence = ref_seq;
            }

            let ref_seq = reference_sequence.expect(
                "reference sequence should either be generated from --fasta by make-splici or set with --ref-seq",
            );

            let input_files = vec![ref_seq.clone()];
            check_files_exist(&input_files)?;

            let output_index_dir = output.join("index");
            let index_duration;
            let index_cmd_string: String;

            if use_piscem {
                // ensure we have piscem
                if rp.piscem.is_none() {
                    bail!("The construction of a piscem index was requested, but a valid piscem executable was not available. \n\
                            Please either set a path using the `set-paths` command, or ensure the `PISCEM` environment variable is set properly.");
                }

                let mut piscem_index_cmd = std::process::Command::new(format!(
                    "{}",
                    rp.piscem.unwrap().exe_path.display()
                ));

                run_fun!(mkdir -p $output_index_dir)?;
                let output_index_stem = output_index_dir.join("piscem_idx");

                piscem_index_cmd
                    .arg("build")
                    .arg("-k")
                    .arg(kmer_length.to_string())
                    .arg("-m")
                    .arg(minimizer_length.to_string())
                    .arg("-o")
                    .arg(&output_index_stem)
                    .arg("-s")
                    .arg(&ref_seq);

                // if the user requested to overwrite, then pass this option
                if overwrite {
                    info!("will attempt to overwrite any existing piscem index, as requested");
                    piscem_index_cmd.arg("--overwrite");
                }

                // if the user requested more threads than can be used
                if let Ok(max_threads_usize) = std::thread::available_parallelism() {
                    let max_threads = max_threads_usize.get() as u32;
                    if threads > max_threads {
                        warn!(
                                "The maximum available parallelism is {}, but {} threads were requested.",
                                max_threads, threads
                            );
                        warn!("setting number of threads to {}", max_threads);
                        threads = max_threads;
                    }
                }

                piscem_index_cmd
                    .arg("--threads")
                    .arg(format!("{}", threads));

                // print piscem build command
                index_cmd_string = get_cmd_line_string(&piscem_index_cmd);
                info!("piscem build cmd : {}", index_cmd_string);

                let index_start = Instant::now();
                let cres = prog_utils::execute_command(
                    &mut piscem_index_cmd,
                    CommandVerbosityLevel::Quiet,
                )
                .expect("failed to invoke piscem index command");
                index_duration = index_start.elapsed();

                if !cres.status.success() {
                    bail!("piscem index failed to build succesfully {:?}", cres.status);
                }

                // copy over the t2g file to the index
                let mut t2g_out_path: Option<PathBuf> = None;
                if let Some(t2g_file) = splici_t2g {
                    let index_t2g_path = output_index_dir.join("t2g_3col.tsv");
                    t2g_out_path = Some(PathBuf::from("t2g_3col.tsv"));
                    std::fs::copy(t2g_file, index_t2g_path)?;
                }

                let index_json_file = output_index_dir.join("simpleaf_index.json");
                let index_json = json!({
                        "cmd" : index_cmd_string,                        "index_type" : "piscem",
                        "t2g_file" : t2g_out_path,
                        "piscem_index_parameters" : {
                            "k" : kmer_length,
                            "m" : minimizer_length,
                            "overwrite" : overwrite,
                            "threads" : threads,
                            "ref" : ref_seq
                        }
                });
                std::fs::write(
                    &index_json_file,
                    serde_json::to_string_pretty(&index_json).unwrap(),
                )
                .with_context(|| format!("could not write {}", index_json_file.display()))?;
            } else {
                // ensure we have piscem
                if rp.salmon.is_none() {
                    bail!("The construction of a salmon index was requested, but a valid piscem executable was not available. \n\
                           Please either set a path using the `simpleaf set-paths` command, or ensure the `SALMON` environment variable is set properly.");
                }

                let mut salmon_index_cmd = std::process::Command::new(format!(
                    "{}",
                    rp.salmon.unwrap().exe_path.display()
                ));

                salmon_index_cmd
                    .arg("index")
                    .arg("-k")
                    .arg(kmer_length.to_string())
                    .arg("-i")
                    .arg(&output_index_dir)
                    .arg("-t")
                    .arg(&ref_seq);

                // overwrite doesn't do anything special for the salmon index, so mention this to
                // the user.
                if overwrite {
                    info!("As the default salmon behavior is to overwrite an existing index if the same directory is provided, \n\
                        the --overwrite flag will have no additional effect.");
                }

                // if the user requested a sparse index.
                if sparse {
                    salmon_index_cmd.arg("--sparse");
                }

                // if the user requested keeping duplicated sequences.
                if keep_duplicates {
                    salmon_index_cmd.arg("--keepDuplicates");
                }

                // if the user requested more threads than can be used
                if let Ok(max_threads_usize) = std::thread::available_parallelism() {
                    let max_threads = max_threads_usize.get() as u32;
                    if threads > max_threads {
                        warn!(
                        "The maximum available parallelism is {}, but {} threads were requested.",
                        max_threads, threads
                    );
                        warn!("setting number of threads to {}", max_threads);
                        threads = max_threads;
                    }
                }

                salmon_index_cmd
                    .arg("--threads")
                    .arg(format!("{}", threads));

                // print salmon index command
                index_cmd_string = get_cmd_line_string(&salmon_index_cmd);
                info!("salmon index cmd : {}", index_cmd_string);

                let index_start = Instant::now();
                let cres = prog_utils::execute_command(
                    &mut salmon_index_cmd,
                    CommandVerbosityLevel::Quiet,
                )
                .expect("failed to invoke salmon index command");
                index_duration = index_start.elapsed();

                if !cres.status.success() {
                    bail!("salmon index failed to build succesfully {:?}", cres.status);
                }

                // copy over the t2g file to the index
                let mut t2g_out_path: Option<PathBuf> = None;
                if let Some(t2g_file) = splici_t2g {
                    let index_t2g_path = output_index_dir.join("t2g_3col.tsv");
                    t2g_out_path = Some(PathBuf::from("t2g_3col.tsv"));
                    std::fs::copy(t2g_file, index_t2g_path)?;
                }

                let index_json_file = output_index_dir.join("simpleaf_index.json");
                let index_json = json!({
                    "cmd" : index_cmd_string,                        "index_type" : "salmon",
                        "t2g_file" : t2g_out_path,
                        "salmon_index_parameters" : {
                            "k" : kmer_length,
                            "overwrite" : overwrite,
                            "sparse" : sparse,
                            "keep_duplicates" : keep_duplicates,
                            "threads" : threads,
                            "ref" : ref_seq
                        }
                });
                std::fs::write(
                    &index_json_file,
                    serde_json::to_string_pretty(&index_json).unwrap(),
                )
                .with_context(|| format!("could not write {}", index_json_file.display()))?;
            }

            let index_log_file = output.join("simpleaf_index_log.json");
            let index_log_info = if let Some(pyroe_duration) = pyroe_duration {
                // if we ran make-splici
                json!({
                    "time_info" : {
                        "pyroe_time" : pyroe_duration,
                        "index_time" : index_duration
                    },
                    "cmd_info" : {
                        "pyroe_cmd" : pyroe_cmd_string,
                        "index_cmd" : index_cmd_string,                    }
                })
            } else {
                // if we indexed provided sequences directly
                json!({
                    "time_info" : {
                        "index_time" : index_duration
                    },
                    "cmd_info" : {
                        "index_cmd" : index_cmd_string,                    }
                })
            };

            std::fs::write(
                &index_log_file,
                serde_json::to_string_pretty(&index_log_info).unwrap(),
            )
            .with_context(|| format!("could not write {}", index_log_file.display()))?;
        }
        _ => {
            bail!("invalid command");
        }
    }
    Ok(())
}

fn inspect_simpleaf(af_home_path: PathBuf) -> anyhow::Result<()> {
    // Read the JSON contents of the file as an instance of `User`.
    let v: serde_json::Value = inspect_af_home(af_home_path.as_path())?;
    println!(
        "\n----- simpleaf info -----\n{}",
        serde_json::to_string_pretty(&v).unwrap()
    );

    // do we have a custom chemistry file
    let custom_chem_p = af_home_path.join("custom_chemistries.json");
    if custom_chem_p.is_file() {
        println!(
            "\nCustom chemistries exist at path: {}\n----- custom chemistries -----\n",
            custom_chem_p.display()
        );
        // parse the custom chemistry json file
        let custom_chem_file = std::fs::File::open(&custom_chem_p).with_context({
            || {
                format!(
                    "couldn't open the custom chemistry file {}",
                    custom_chem_p.display()
                )
            }
        })?;
        let custom_chem_reader = BufReader::new(custom_chem_file);
        let v: serde_json::Value = serde_json::from_reader(custom_chem_reader)?;
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    }
    Ok(())
}

fn add_chemistry(af_home_path: PathBuf, add_chem_cmd: Commands) -> anyhow::Result<()> {
    match add_chem_cmd {
        Commands::AddChemistry { name, geometry } => {
            // check geometry string, if no good then
            // propagate error.
            let _cg = extract_geometry(&geometry)?;

            // do we have a custom chemistry file
            let custom_chem_p = af_home_path.join("custom_chemistries.json");

            let mut custom_chem_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&custom_chem_p)
                .with_context({
                    || {
                        format!(
                            "couldn't open the custom chemistry file {}",
                            custom_chem_p.display()
                        )
                    }
                })?;

            let custom_chem_reader = BufReader::new(&custom_chem_file);
            let mut v: serde_json::Value = match serde_json::from_reader(custom_chem_reader) {
                Ok(sv) => sv,
                Err(_) => {
                    // the file was empty so here return an empty json object
                    json!({})
                }
            };

            if let Some(g) = v.get_mut(&name) {
                let gs = g.as_str().unwrap();
                info!("chemistry {} already existed, with geometry {}; overwriting geometry specification", name, gs);
                *g = json!(geometry);
            } else {
                info!("inserting chemistry {} with geometry {}", name, geometry);
                v[name] = json!(geometry);
            }

            custom_chem_file.set_len(0)?;
            // custom_chem_file.seek(SeekFrom::Start(0))?;
            // suggested by cargo clippy
            custom_chem_file.rewind()?;

            custom_chem_file
                .write_all(serde_json::to_string_pretty(&v).unwrap().as_bytes())
                .with_context(|| format!("could not write {}", custom_chem_p.display()))?;
        }
        _ => {
            bail!("unknown command");
        }
    }
    Ok(())
}

fn map_and_quant(af_home_path: &Path, quant_cmd: Commands) -> anyhow::Result<()> {
    match quant_cmd {
        Commands::Quant {
            index,
            use_piscem,
            map_dir,
            reads1,
            reads2,
            mut threads,
            use_selective_alignment,
            expected_ori,
            knee,
            unfiltered_pl,
            explicit_pl,
            forced_cells,
            expect_cells,
            min_reads,
            resolution,
            mut t2g_map,
            chemistry,
            output,
        } => {
            // Read the JSON contents of the file as an instance of `User`.
            let v: serde_json::Value = inspect_af_home(af_home_path)?;
            let rp: ReqProgs = serde_json::from_value(v["prog_info"].clone())?;

            // info!("prog info = {:?}", rp);

            let mut had_simpleaf_index_json = false;
            let mut index_type_str = String::new();
            if let Some(index) = index.clone() {
                let index_json_path = index.join("simpleaf_index.json");
                match index_json_path.try_exists() {
                    Ok(true) => {
                        // we have the simpleaf_index.json file, so parse it.
                        let index_json_file =
                            std::fs::File::open(&index_json_path).with_context({
                                || format!("Could not open file {}", index_json_path.display())
                            })?;

                        let index_json_reader = BufReader::new(&index_json_file);
                        let v: serde_json::Value = serde_json::from_reader(index_json_reader)?;
                        had_simpleaf_index_json = true;
                        index_type_str = serde_json::from_value(v["index_type"].clone())?;
                        // if the user didn't pass in a t2g_map, try and populate it
                        // automatically here
                        if t2g_map.is_none() {
                            let t2g_opt: Option<PathBuf> =
                                serde_json::from_value(v["t2g_file"].clone())?;
                            if let Some(t2g_val) = t2g_opt {
                                let t2g_loc = index.join(t2g_val);
                                info!("found local t2g file at {}, will attempt to use this since none was provided explicitly", t2g_loc.display());
                                t2g_map = Some(t2g_loc);
                            }
                        }
                    }
                    Ok(false) => {
                        had_simpleaf_index_json = false;
                    }
                    Err(e) => {
                        bail!(e);
                    }
                }
            }

            // at this point make sure we have a t2g value
            let t2g_map_file = t2g_map.context("A transcript-to-gene map (t2g) file was not provided via `--t2g-map`|`-m` and could \
                    not be inferred from the index. Please provide a t2g map explicitly to the quant command.")?;
            check_files_exist(&[t2g_map_file.clone()])?;

            // figure out what type of index we expect
            let index_type;
            // only bother with this if we are mapping reads and not if we are
            // starting from a RAD file
            if let Some(index) = index.clone() {
                // if the user said piscem explicitly, believe them
                if !use_piscem {
                    if had_simpleaf_index_json {
                        match index_type_str.as_ref() {
                            "salmon" => {
                                index_type = IndexType::Salmon(index);
                            }
                            "piscem" => {
                                index_type = IndexType::Piscem(index.join("piscem_idx"));
                            }
                            _ => {
                                bail!(
                                    "unknown index type {} present in simpleaf_index.json",
                                    index_type_str,
                                );
                            }
                        }
                    } else {
                        index_type = IndexType::Salmon(index);
                    }
                } else {
                    index_type = IndexType::Piscem(index);
                }
            } else {
                index_type = IndexType::NoIndex;
            }

            // make sure we have an program matching the
            // appropriate index type
            match index_type {
                IndexType::Piscem(_) => {
                    if rp.piscem.is_none() {
                        bail!("A piscem index is being used, but no piscem executable is provided. Please set one with `simpleaf set-paths`.");
                    }
                }
                IndexType::Salmon(_) => {
                    if rp.salmon.is_none() {
                        bail!("A salmon index is being used, but no piscem executable is provided. Please set one with `simpleaf set-paths`.");
                    }
                }
                IndexType::NoIndex => {}
            }

            // do we have a custom chemistry file
            let custom_chem_p = af_home_path.join("custom_chemistries.json");
            let custom_chem_exists = custom_chem_p.is_file();

            let chem = match chemistry.as_str() {
                "10xv2" => Chemistry::TenxV2,
                "10xv3" => Chemistry::TenxV3,
                s => {
                    if custom_chem_exists {
                        // parse the custom chemistry json file
                        let custom_chem_file =
                            std::fs::File::open(&custom_chem_p).with_context({
                                || {
                                    format!(
                                        "couldn't open the custom chemistry file {}",
                                        custom_chem_p.display()
                                    )
                                }
                            })?;
                        let custom_chem_reader = BufReader::new(custom_chem_file);
                        let v: serde_json::Value = serde_json::from_reader(custom_chem_reader)?;
                        let rchem = match v[s.to_string()].as_str() {
                            Some(chem_str) => {
                                info!("custom chemistry {} maps to geometry {}", s, &chem_str);
                                Chemistry::Other(chem_str.to_string())
                            }
                            None => Chemistry::Other(s.to_string()),
                        };
                        rchem
                    } else {
                        // pass along whatever the user gave us
                        Chemistry::Other(s.to_string())
                    }
                }
            };

            let ori;
            // if the user set the orientation, then
            // use that explicitly
            if let Some(o) = expected_ori {
                ori = o;
            } else {
                // otherwise, this was not set explicitly. In that case
                // if we have 10xv2 or 10xv3 chemistry, set ori = "fw"
                // otherwise set ori = "both"
                match chem {
                    Chemistry::TenxV2 | Chemistry::TenxV3 => {
                        ori = "fw".to_string();
                    }
                    _ => {
                        ori = "both".to_string();
                    }
                }
            }

            let mut filter_meth_opt = None;

            // based on the filtering method
            if let Some(pl_file) = unfiltered_pl {
                // NOTE: unfiltered_pl is of type Option<Option<PathBuf>> so being in here
                // tells us nothing about the inner option.  We handle that now.

                // if the -u flag is passed and some file is provided, then the inner
                // Option is Some(PathBuf)
                if let Some(pl_file) = pl_file {
                    // the user has explicily passed a file along, so try
                    // to use that
                    if pl_file.is_file() {
                        let min_cells = min_reads;
                        filter_meth_opt = Some(CellFilterMethod::UnfilteredExternalList(
                            pl_file.to_string_lossy().into_owned(),
                            min_cells,
                        ));
                    } else {
                        bail!(
                            "The provided path {} does not exist as a regular file.",
                            pl_file.display()
                        );
                    }
                } else {
                    // here, the -u flag is provided
                    // but no file is provided, then the
                    // inner option is None and we will try to get the permit list automatically if
                    // using 10xv2 or 10xv3

                    // check the chemistry
                    let pl_res = get_permit_if_absent(af_home_path, &chem)?;
                    let min_cells = min_reads;
                    match pl_res {
                        PermitListResult::DownloadSuccessful(p)
                        | PermitListResult::AlreadyPresent(p) => {
                            filter_meth_opt = Some(CellFilterMethod::UnfilteredExternalList(
                                p.to_string_lossy().into_owned(),
                                min_cells,
                            ));
                        }
                        PermitListResult::UnregisteredChemistry => {
                            bail!(
                                    "Cannot automatically obtain an unfiltered permit list for non-Chromium chemistry: {}.",
                                    chem.as_str()
                                    );
                        }
                    }
                }
            } else {
                if let Some(filtered_path) = explicit_pl {
                    filter_meth_opt = Some(CellFilterMethod::ExplicitList(
                        filtered_path.to_string_lossy().into_owned(),
                    ));
                };
                if let Some(num_forced) = forced_cells {
                    filter_meth_opt = Some(CellFilterMethod::ForceCells(num_forced));
                };
                if let Some(num_expected) = expect_cells {
                    filter_meth_opt = Some(CellFilterMethod::ExpectCells(num_expected));
                };
            }
            // otherwise it must have been knee;
            if knee {
                filter_meth_opt = Some(CellFilterMethod::KneeFinding);
            }

            if filter_meth_opt.is_none() {
                bail!("No valid filtering strategy was provided!");
            }

            // if the user requested more threads than can be used
            if let Ok(max_threads_usize) = std::thread::available_parallelism() {
                let max_threads = max_threads_usize.get() as u32;
                if threads > max_threads {
                    warn!(
                        "The maximum available parallelism is {}, but {} threads were requested.",
                        max_threads, threads
                    );
                    warn!("setting number of threads to {}", max_threads);
                    threads = max_threads;
                }
            }

            // here we must be safe to unwrap
            let filter_meth = filter_meth_opt.unwrap();

            let sc_mapper: String;
            let map_cmd_string: String;
            let map_output: PathBuf;
            let map_duration: Duration;

            // if we are mapping against an index
            if let Some(index) = index {
                let reads1 = reads1.expect(
                    "since mapping against an index is requested, read1 files must be provided.",
                );
                let reads2 = reads2.expect(
                    "since mapping against an index is requested, read2 files must be provided.",
                );
                assert_eq!(
                    reads1.len(),
                    reads2.len(),
                    "{} read1 files and {} read2 files were given; Cannot proceed!",
                    reads1.len(),
                    reads2.len()
                );

                match index_type {
                    IndexType::Piscem(index_base) => {
                        // using a piscem index
                        let mut piscem_quant_cmd = std::process::Command::new(format!(
                            "{}",
                            rp.piscem.unwrap().exe_path.display()
                        ));
                        let index_path = format!("{}", index_base.display());
                        piscem_quant_cmd
                            .arg("map-sc")
                            .arg("--index")
                            .arg(index_path);

                        // location of output directory, number of threads
                        map_output = output.join("af_map");
                        piscem_quant_cmd
                            .arg("--threads")
                            .arg(format!("{}", threads))
                            .arg("-o")
                            .arg(&map_output);

                        let reads1_str = reads1
                            .iter()
                            .map(|x| x.to_string_lossy().into_owned())
                            .collect::<Vec<String>>()
                            .join(",");
                        piscem_quant_cmd.arg("-1").arg(reads1_str);

                        let reads2_str = reads2
                            .iter()
                            .map(|x| x.to_string_lossy().into_owned())
                            .collect::<Vec<String>>()
                            .join(",");
                        piscem_quant_cmd.arg("-2").arg(reads2_str);

                        // setting the technology / chemistry
                        add_chemistry_to_args_piscem(chem.as_str(), &mut piscem_quant_cmd)?;

                        map_cmd_string = get_cmd_line_string(&piscem_quant_cmd);
                        info!("piscem map-sc cmd : {}", map_cmd_string);
                        sc_mapper = String::from("piscem");

                        let mut input_files = vec![
                            index_base.with_extension("ctab"),
                            index_base.with_extension("refinfo"),
                            index_base.with_extension("sshash"),
                        ];
                        input_files.extend_from_slice(&reads1);
                        input_files.extend_from_slice(&reads2);

                        check_files_exist(&input_files)?;

                        let map_start = Instant::now();
                        let cres = prog_utils::execute_command(
                            &mut piscem_quant_cmd,
                            CommandVerbosityLevel::Quiet,
                        )
                        .expect("failed to execute piscem [mapping phase]");
                        map_duration = map_start.elapsed();

                        if !cres.status.success() {
                            bail!("piscem mapping failed with exit status {:?}", cres.status);
                        }
                    }
                    IndexType::Salmon(index_base) => {
                        // using a salmon index
                        let mut salmon_quant_cmd = std::process::Command::new(format!(
                            "{}",
                            rp.salmon.unwrap().exe_path.display()
                        ));

                        // set the input index and library type
                        let index_path = format!("{}", index_base.display());
                        salmon_quant_cmd
                            .arg("alevin")
                            .arg("--index")
                            .arg(index_path)
                            .arg("-l")
                            .arg("A");

                        // location of the reads
                        // note: salmon uses space so separate
                        // these, not commas, so build the proper
                        // strings here.

                        salmon_quant_cmd.arg("-1");
                        for rf in &reads1 {
                            salmon_quant_cmd.arg(rf);
                        }
                        salmon_quant_cmd.arg("-2");
                        for rf in &reads2 {
                            salmon_quant_cmd.arg(rf);
                        }

                        // location of output directory, number of threads
                        map_output = output.join("af_map");
                        salmon_quant_cmd
                            .arg("--threads")
                            .arg(format!("{}", threads))
                            .arg("-o")
                            .arg(&map_output);

                        // if the user explicitly requested to use selective-alignment
                        // then enable that
                        if use_selective_alignment {
                            salmon_quant_cmd.arg("--rad");
                        } else {
                            // otherwise default to sketch mode
                            salmon_quant_cmd.arg("--sketch");
                        }

                        // setting the technology / chemistry
                        add_chemistry_to_args_salmon(chem.as_str(), &mut salmon_quant_cmd)?;

                        map_cmd_string = get_cmd_line_string(&salmon_quant_cmd);
                        info!("salmon alevin cmd : {}", map_cmd_string);
                        sc_mapper = String::from("salmon");

                        let mut input_files = vec![index];
                        input_files.extend_from_slice(&reads1);
                        input_files.extend_from_slice(&reads2);

                        check_files_exist(&input_files)?;

                        let map_start = Instant::now();
                        let cres = prog_utils::execute_command(
                            &mut salmon_quant_cmd,
                            CommandVerbosityLevel::Quiet,
                        )
                        .expect("failed to execute salmon [mapping phase]");
                        map_duration = map_start.elapsed();

                        if !cres.status.success() {
                            bail!("salmon mapping failed with exit status {:?}", cres.status);
                        }
                    }
                    IndexType::NoIndex => {
                        bail!("Cannot perform mapping an quantification without known (piscem or salmon) index!");
                    }
                }
            } else {
                map_cmd_string = String::from("");
                sc_mapper = String::from("");
                map_output = map_dir
                    .expect("map-dir must be provided, since index, read1 and read2 were not.");
                map_duration = Duration::new(0, 0);
            }

            let map_output_string = map_output.display().to_string();

            let alevin_fry = rp.alevin_fry.unwrap().exe_path;
            // alevin-fry generate permit list
            let mut alevin_gpl_cmd =
                std::process::Command::new(format!("{}", &alevin_fry.display()));

            alevin_gpl_cmd.arg("generate-permit-list");
            alevin_gpl_cmd.arg("-i").arg(&map_output);
            alevin_gpl_cmd.arg("-d").arg(&ori);

            // add the filter mode
            add_to_args(&filter_meth, &mut alevin_gpl_cmd);

            let gpl_output = output.join("af_quant");
            alevin_gpl_cmd.arg("-o").arg(&gpl_output);

            info!(
                "alevin-fry generate-permit-list cmd : {}",
                get_cmd_line_string(&alevin_gpl_cmd)
            );
            let input_files = vec![map_output.clone()];
            check_files_exist(&input_files)?;

            let gpl_start = Instant::now();
            let gpl_proc_out =
                prog_utils::execute_command(&mut alevin_gpl_cmd, CommandVerbosityLevel::Quiet)
                    .expect("could not execute [generate permit list]");
            let gpl_duration = gpl_start.elapsed();

            if !gpl_proc_out.status.success() {
                bail!(
                    "alevin-fry generate-permit-list failed with exit status {:?}",
                    gpl_proc_out.status
                );
            }

            //
            // collate
            //
            let mut alevin_collate_cmd =
                std::process::Command::new(format!("{}", &alevin_fry.display()));

            alevin_collate_cmd.arg("collate");
            alevin_collate_cmd.arg("-i").arg(&gpl_output);
            alevin_collate_cmd.arg("-r").arg(&map_output);
            alevin_collate_cmd.arg("-t").arg(format!("{}", threads));

            info!(
                "alevin-fry collate cmd : {}",
                get_cmd_line_string(&alevin_collate_cmd)
            );
            let input_files = vec![gpl_output.clone(), map_output];
            check_files_exist(&input_files)?;

            let collate_start = Instant::now();
            let collate_proc_out =
                prog_utils::execute_command(&mut alevin_collate_cmd, CommandVerbosityLevel::Quiet)
                    .expect("could not execute [collate]");
            let collate_duration = collate_start.elapsed();

            if !collate_proc_out.status.success() {
                bail!(
                    "alevin-fry collate failed with exit status {:?}",
                    collate_proc_out.status
                );
            }

            //
            // quant
            //
            let mut alevin_quant_cmd =
                std::process::Command::new(format!("{}", &alevin_fry.display()));

            alevin_quant_cmd
                .arg("quant")
                .arg("-i")
                .arg(&gpl_output)
                .arg("-o")
                .arg(&gpl_output);
            alevin_quant_cmd.arg("-t").arg(format!("{}", threads));
            alevin_quant_cmd.arg("-m").arg(t2g_map_file.clone());
            alevin_quant_cmd.arg("-r").arg(resolution);

            info!("cmd : {:?}", alevin_quant_cmd);

            let input_files = vec![gpl_output, t2g_map_file];
            check_files_exist(&input_files)?;

            let quant_start = Instant::now();
            let quant_proc_out =
                prog_utils::execute_command(&mut alevin_quant_cmd, CommandVerbosityLevel::Quiet)
                    .expect("could not execute [quant]");
            let quant_duration = quant_start.elapsed();

            if !quant_proc_out.status.success() {
                bail!("quant failed with exit status {:?}", quant_proc_out.status);
            }

            let af_quant_info_file = output.join("simpleaf_quant_log.json");
            let af_quant_info = json!({
                "time_info" : {
                    "map_time" : map_duration,
                    "gpl_time" : gpl_duration,
                    "collate_time" : collate_duration,
                    "quant_time" : quant_duration
                },
                "cmd_info" : {
                    "map_cmd" : map_cmd_string,
                    "gpl_cmd" : get_cmd_line_string(&alevin_gpl_cmd),
                    "collate_cmd" : get_cmd_line_string(&alevin_gpl_cmd),
                    "quant_cmd" : get_cmd_line_string(&alevin_quant_cmd)
                    },
                "map_info" : {
                    "mapper" : sc_mapper,
                    "map_cmd" : map_cmd_string,
                    "map_outdir": map_output_string
                    }
            });

            // write the relevant info about
            // our run to file.
            std::fs::write(
                &af_quant_info_file,
                serde_json::to_string_pretty(&af_quant_info).unwrap(),
            )
            .with_context(|| format!("could not write {}", af_quant_info_file.display()))?;
        }
        _ => {
            bail!("unknown command")
        }
    }
    Ok(())
}

// Program Name: simpleaf generate-workflow
// Program Input: a json file that records all top level variables needed by the template
//                  and optionally, some extra variables
// Program Output: a json file that contains the actual simpelaf workflow information, which can be
//         consumed directly by the simpleaf execute-workflow command.

// This crate is used for generating a simpleaf workflow JSON file
// that can be consumed directly by the `simpleaf workflow` program.
// Thir program takes a template from our template library as the input
// and do the following:
// 1. It loads the required arguments of that template and
//      find them in the user-provided JSON file.
// 2. It validates the files in the user-provided JSON file.
//      This can be checking the existance and validate the first few records
// 3. It feeds the template the required inputs, and
//      generates a simpleaf workflow JSON file.
//      This JSON file contains the simpleaf programs need to be run and
//      the required arguments.



fn run_workflow(af_home_path: PathBuf, rw_args: Commands) -> anyhow::Result<()> {
    match rw_args {
        Commands::RunWorkflow { jsons } => {
            //  check the validity of the JSON file
            check_files_exist(&jsons)?;

            info!("Parsing provided simpleaf workflow JSON files");
            // define vectors for index and quant commands
            let mut index_cmd_v: Vec<Cli> = Vec::new();
            index_cmd_v.reserve(jsons.len());

            let mut quant_cmd_v: Vec<Cli> = Vec::new();
            quant_cmd_v.reserve(jsons.len());

            // iterate json files and parse records to commands
            for jf in jsons {
                let json_records = read_workflow_json(&jf)?;

                // process simpleaf index command records if any
                if let Some(index_records) = json_records.index {
                    for (index_name, index_record) in index_records {
                        info!("processing simpleaf index - {}", index_name);
                        if let Some(cmd_string) = index_record.get("cmd") {
                            let cmd_vec: Vec<String> = cmd_string
                                .to_string()
                                .trim_matches('"')
                                .split_whitespace()
                                .map(|x| x.to_string())
                                .collect();
                            let parsed_cmd = Cli::parse_from(cmd_vec);
                            index_cmd_v.push(parsed_cmd);
                        }
                    }
                }

                // process simpleaf quant command records if any
                if let Some(quant_records) = json_records.quant {
                    for (quant_name, quant_record) in quant_records {
                        info!("processing simpleaf quant - {}", quant_name);
                        if let Some(cmd_string) = quant_record.get("cmd") {
                            let cmd_vec: Vec<String> = cmd_string
                                .to_string()
                                .trim_matches('"')
                                .split_whitespace()
                                .map(|x| x.to_string())
                                .collect();
                            let parsed_cmd = Cli::parse_from(cmd_vec);
                            quant_cmd_v.push(parsed_cmd);
                        }
                    }
                }
            }

            info!(
                "Found {} simpleaf index commands and {} simpleaf quant commands",
                index_cmd_v.len(),
                quant_cmd_v.len()
            );

            info!("Running commands");

            // run simpleaf index commands
            for index_cmd in index_cmd_v {
                build_ref_and_index(af_home_path.as_path(), index_cmd.command)?;
            }

            // run simpleaf quant commands
            for quant_cmd in quant_cmd_v {
                build_ref_and_index(af_home_path.as_path(), quant_cmd.command)?;
            }
        }
        _ => {
            bail!("unknown command")
        }
    }
    Ok(())
}

enum IndexType {
    Salmon(PathBuf),
    Piscem(PathBuf),
    NoIndex,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();
    const AF_HOME: &str = "ALEVIN_FRY_HOME";
    let af_home_path = match env::var(AF_HOME) {
        Ok(p) => PathBuf::from(p),
        Err(e) => {
            bail!(
                "${} is unset {}, please set this environment variable to continue.",
                AF_HOME,
                e
            );
        }
    };

    let cli_args = Cli::parse_from([
        "simplef",
        "run-workflow",
        "-j",
        "/mnt/scratch5/dongze/CODE/rust/playground/CITE-seq.json",
    ]);

    match cli_args.command {
        // set the paths where the relevant tools live
        Commands::SetPaths {
            salmon,
            piscem,
            alevin_fry,
            pyroe,
        } => set_paths(
            af_home_path,
            Commands::SetPaths {
                salmon,
                piscem,
                alevin_fry,
                pyroe,
            },
        ),
        Commands::AddChemistry { name, geometry } => {
            add_chemistry(af_home_path, Commands::AddChemistry { name, geometry })
        }
        Commands::Inspect {} => inspect_simpleaf(af_home_path),
        // if we are building the reference and indexing
        Commands::Index {
            ref_type,
            fasta,
            gtf,
            rlen,
            spliced,
            unspliced,
            dedup,
            keep_duplicates,
            ref_seq,
            output,
            use_piscem,
            kmer_length,
            minimizer_length,
            overwrite,
            sparse,
            threads,
        } => build_ref_and_index(
            af_home_path.as_path(),
            Commands::Index {
                ref_type,
                fasta,
                gtf,
                rlen,
                spliced,
                unspliced,
                dedup,
                keep_duplicates,
                ref_seq,
                output,
                use_piscem,
                kmer_length,
                minimizer_length,
                overwrite,
                sparse,
                threads,
            },
        ),

        // if we are running mapping and quantification
        Commands::Quant {
            index,
            use_piscem,
            map_dir,
            reads1,
            reads2,
            threads,
            use_selective_alignment,
            expected_ori,
            knee,
            unfiltered_pl,
            explicit_pl,
            forced_cells,
            expect_cells,
            min_reads,
            resolution,
            t2g_map,
            chemistry,
            output,
        } => map_and_quant(
            af_home_path.as_path(),
            Commands::Quant {
                index,
                use_piscem,
                map_dir,
                reads1,
                reads2,
                threads,
                use_selective_alignment,
                expected_ori,
                knee,
                unfiltered_pl,
                explicit_pl,
                forced_cells,
                expect_cells,
                min_reads,
                resolution,
                t2g_map,
                chemistry,
                output,
            },
        ),
        Commands::RunWorkflow { jsons } => {
            run_workflow(af_home_path, Commands::RunWorkflow { jsons })
        }
    }
    // success, yay!
}
