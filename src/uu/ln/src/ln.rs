//  * This file is part of the uutils coreutils package.
//  *
//  * (c) Joseph Crail <jbcrail@gmail.com>
//  *
//  * For the full copyright and license information, please view the LICENSE
//  * file that was distributed with this source code.

// spell-checker:ignore (ToDO) srcpath targetpath EEXIST

#[macro_use]
extern crate uucore;

use clap::{crate_version, App, Arg};
use uucore::error::{UCustomError, UResult};

use std::borrow::Cow;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt::Display;
use std::fs;

use std::io::{stdin, Result};
#[cfg(any(unix, target_os = "redox"))]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};
use std::path::{Path, PathBuf};
use uucore::backup_control::{self, BackupMode};
use uucore::fs::{canonicalize, CanonicalizeMode};

pub struct Settings {
    overwrite: OverwriteMode,
    backup: BackupMode,
    suffix: String,
    symbolic: bool,
    relative: bool,
    target_dir: Option<String>,
    no_target_dir: bool,
    no_dereference: bool,
    verbose: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OverwriteMode {
    NoClobber,
    Interactive,
    Force,
}

#[derive(Debug)]
enum LnError {
    TargetIsDirectory(String),
    SomeLinksFailed,
    FailedToLink(String),
    MissingDestination(String),
    ExtraOperand(String),
    InvalidBackupMode(String),
}

impl Display for LnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetIsDirectory(s) => write!(f, "target '{}' is not a directory", s),
            Self::FailedToLink(s) => write!(f, "failed to link '{}'", s),
            Self::SomeLinksFailed => write!(f, "some links failed to create"),
            Self::MissingDestination(s) => {
                write!(f, "missing destination file operand after '{}'", s)
            }
            Self::ExtraOperand(s) => write!(
                f,
                "extra operand '{}'\nTry '{} --help' for more information.",
                s,
                executable!()
            ),
            Self::InvalidBackupMode(s) => write!(f, "{}", s),
        }
    }
}

impl Error for LnError {}

impl UCustomError for LnError {
    fn code(&self) -> i32 {
        match self {
            Self::TargetIsDirectory(_) => 1,
            Self::SomeLinksFailed => 1,
            Self::FailedToLink(_) => 1,
            Self::MissingDestination(_) => 1,
            Self::ExtraOperand(_) => 1,
            Self::InvalidBackupMode(_) => 1,
        }
    }
}

fn get_usage() -> String {
    format!(
        "{0} [OPTION]... [-T] TARGET LINK_NAME   (1st form)
       {0} [OPTION]... TARGET                  (2nd form)
       {0} [OPTION]... TARGET... DIRECTORY     (3rd form)
       {0} [OPTION]... -t DIRECTORY TARGET...  (4th form)",
        executable!()
    )
}

fn get_long_usage() -> String {
    String::from(
        " In the 1st form, create a link to TARGET with the name LINK_NAME.
        In the 2nd form, create a link to TARGET in the current directory.
        In the 3rd and 4th forms, create links to each TARGET in DIRECTORY.
        Create hard links by default, symbolic links with --symbolic.
        By default, each destination (name of new link) should not already exist.
        When creating hard links, each TARGET must exist.  Symbolic links
        can hold arbitrary text; if later resolved, a relative link is
        interpreted in relation to its parent directory.
        ",
    )
}

static ABOUT: &str = "change file owner and group";

mod options {
    pub const BACKUP_NO_ARG: &str = "b";
    pub const BACKUP: &str = "backup";
    pub const FORCE: &str = "force";
    pub const INTERACTIVE: &str = "interactive";
    pub const NO_DEREFERENCE: &str = "no-dereference";
    pub const SYMBOLIC: &str = "symbolic";
    pub const SUFFIX: &str = "suffix";
    pub const TARGET_DIRECTORY: &str = "target-directory";
    pub const NO_TARGET_DIRECTORY: &str = "no-target-directory";
    pub const RELATIVE: &str = "relative";
    pub const VERBOSE: &str = "verbose";
}

static ARG_FILES: &str = "files";

#[uucore_procs::gen_uumain]
pub fn uumain(args: impl uucore::Args) -> UResult<()> {
    let usage = get_usage();
    let long_usage = get_long_usage();

    let matches = uu_app()
        .usage(&usage[..])
        .after_help(&*format!(
            "{}\n{}",
            long_usage,
            backup_control::BACKUP_CONTROL_LONG_HELP
        ))
        .get_matches_from(args);

    /* the list of files */

    let paths: Vec<PathBuf> = matches
        .values_of(ARG_FILES)
        .unwrap()
        .map(PathBuf::from)
        .collect();

    let overwrite_mode = if matches.is_present(options::FORCE) {
        OverwriteMode::Force
    } else if matches.is_present(options::INTERACTIVE) {
        OverwriteMode::Interactive
    } else {
        OverwriteMode::NoClobber
    };

    let backup_mode = backup_control::determine_backup_mode(
        matches.is_present(options::BACKUP_NO_ARG),
        matches.is_present(options::BACKUP),
        matches.value_of(options::BACKUP),
    );
    let backup_mode = match backup_mode {
        Err(err) => {
            return Err(LnError::InvalidBackupMode(err).into());
        }
        Ok(mode) => mode,
    };

    let backup_suffix = backup_control::determine_backup_suffix(matches.value_of(options::SUFFIX));

    let settings = Settings {
        overwrite: overwrite_mode,
        backup: backup_mode,
        suffix: backup_suffix,
        symbolic: matches.is_present(options::SYMBOLIC),
        relative: matches.is_present(options::RELATIVE),
        target_dir: matches
            .value_of(options::TARGET_DIRECTORY)
            .map(String::from),
        no_target_dir: matches.is_present(options::NO_TARGET_DIRECTORY),
        no_dereference: matches.is_present(options::NO_DEREFERENCE),
        verbose: matches.is_present(options::VERBOSE),
    };

    exec(&paths[..], &settings)
}

pub fn uu_app() -> App<'static, 'static> {
    App::new(executable!())
        .version(crate_version!())
        .about(ABOUT)
        .arg(
            Arg::with_name(options::BACKUP)
                .long(options::BACKUP)
                .help("make a backup of each existing destination file")
                .takes_value(true)
                .require_equals(true)
                .min_values(0)
                .value_name("CONTROL"),
        )
        .arg(
            Arg::with_name(options::BACKUP_NO_ARG)
                .short(options::BACKUP_NO_ARG)
                .help("like --backup but does not accept an argument"),
        )
        // TODO: opts.arg(
        //    Arg::with_name(("d", "directory", "allow users with appropriate privileges to attempt \
        //                                       to make hard links to directories");
        .arg(
            Arg::with_name(options::FORCE)
                .short("f")
                .long(options::FORCE)
                .help("remove existing destination files"),
        )
        .arg(
            Arg::with_name(options::INTERACTIVE)
                .short("i")
                .long(options::INTERACTIVE)
                .help("prompt whether to remove existing destination files"),
        )
        .arg(
            Arg::with_name(options::NO_DEREFERENCE)
                .short("n")
                .long(options::NO_DEREFERENCE)
                .help(
                    "treat LINK_NAME as a normal file if it is a \
                     symbolic link to a directory",
                ),
        )
        // TODO: opts.arg(
        //    Arg::with_name(("L", "logical", "dereference TARGETs that are symbolic links");
        //
        // TODO: opts.arg(
        //    Arg::with_name(("P", "physical", "make hard links directly to symbolic links");
        .arg(
            Arg::with_name(options::SYMBOLIC)
                .short("s")
                .long("symbolic")
                .help("make symbolic links instead of hard links")
                // override added for https://github.com/uutils/coreutils/issues/2359
                .overrides_with(options::SYMBOLIC),
        )
        .arg(
            Arg::with_name(options::SUFFIX)
                .short("S")
                .long(options::SUFFIX)
                .help("override the usual backup suffix")
                .value_name("SUFFIX")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(options::TARGET_DIRECTORY)
                .short("t")
                .long(options::TARGET_DIRECTORY)
                .help("specify the DIRECTORY in which to create the links")
                .value_name("DIRECTORY")
                .conflicts_with(options::NO_TARGET_DIRECTORY),
        )
        .arg(
            Arg::with_name(options::NO_TARGET_DIRECTORY)
                .short("T")
                .long(options::NO_TARGET_DIRECTORY)
                .help("treat LINK_NAME as a normal file always"),
        )
        .arg(
            Arg::with_name(options::RELATIVE)
                .short("r")
                .long(options::RELATIVE)
                .help("create symbolic links relative to link location")
                .requires(options::SYMBOLIC),
        )
        .arg(
            Arg::with_name(options::VERBOSE)
                .short("v")
                .long(options::VERBOSE)
                .help("print name of each linked file"),
        )
        .arg(
            Arg::with_name(ARG_FILES)
                .multiple(true)
                .takes_value(true)
                .required(true)
                .min_values(1),
        )
}

fn exec(files: &[PathBuf], settings: &Settings) -> UResult<()> {
    // Handle cases where we create links in a directory first.
    if let Some(ref name) = settings.target_dir {
        // 4th form: a directory is specified by -t.
        return link_files_in_dir(files, &PathBuf::from(name), settings);
    }
    if !settings.no_target_dir {
        if files.len() == 1 {
            // 2nd form: the target directory is the current directory.
            return link_files_in_dir(files, &PathBuf::from("."), settings);
        }
        let last_file = &PathBuf::from(files.last().unwrap());
        if files.len() > 2 || last_file.is_dir() {
            // 3rd form: create links in the last argument.
            return link_files_in_dir(&files[0..files.len() - 1], last_file, settings);
        }
    }

    // 1st form. Now there should be only two operands, but if -T is
    // specified we may have a wrong number of operands.
    if files.len() == 1 {
        return Err(LnError::MissingDestination(files[0].to_string_lossy().into()).into());
    }
    if files.len() > 2 {
        return Err(LnError::ExtraOperand(files[2].display().to_string()).into());
    }
    assert!(!files.is_empty());

    match link(&files[0], &files[1], settings) {
        Ok(_) => Ok(()),
        Err(e) => Err(LnError::FailedToLink(e.to_string()).into()),
    }
}

fn link_files_in_dir(files: &[PathBuf], target_dir: &Path, settings: &Settings) -> UResult<()> {
    if !target_dir.is_dir() {
        return Err(LnError::TargetIsDirectory(target_dir.display().to_string()).into());
    }

    let mut all_successful = true;
    for srcpath in files.iter() {
        let targetpath =
            if settings.no_dereference && matches!(settings.overwrite, OverwriteMode::Force) {
                // In that case, we don't want to do link resolution
                // We need to clean the target
                if is_symlink(target_dir) {
                    if target_dir.is_file() {
                        if let Err(e) = fs::remove_file(target_dir) {
                            show_error!("Could not update {}: {}", target_dir.display(), e)
                        };
                    }
                    if target_dir.is_dir() {
                        // Not sure why but on Windows, the symlink can be
                        // considered as a dir
                        // See test_ln::test_symlink_no_deref_dir
                        if let Err(e) = fs::remove_dir(target_dir) {
                            show_error!("Could not update {}: {}", target_dir.display(), e)
                        };
                    }
                }
                target_dir.to_path_buf()
            } else {
                match srcpath.as_os_str().to_str() {
                    Some(name) => {
                        match Path::new(name).file_name() {
                            Some(basename) => target_dir.join(basename),
                            // This can be None only for "." or "..". Trying
                            // to create a link with such name will fail with
                            // EEXIST, which agrees with the behavior of GNU
                            // coreutils.
                            None => target_dir.join(name),
                        }
                    }
                    None => {
                        show_error!(
                            "cannot stat '{}': No such file or directory",
                            srcpath.display()
                        );
                        all_successful = false;
                        continue;
                    }
                }
            };

        if let Err(e) = link(srcpath, &targetpath, settings) {
            show_error!(
                "cannot link '{}' to '{}': {}",
                targetpath.display(),
                srcpath.display(),
                e
            );
            all_successful = false;
        }
    }
    if all_successful {
        Ok(())
    } else {
        Err(LnError::SomeLinksFailed.into())
    }
}

fn relative_path<'a>(src: &Path, dst: &Path) -> Result<Cow<'a, Path>> {
    let src_abs = canonicalize(src, CanonicalizeMode::Normal)?;
    let mut dst_abs = canonicalize(dst.parent().unwrap(), CanonicalizeMode::Normal)?;
    dst_abs.push(dst.components().last().unwrap());
    let suffix_pos = src_abs
        .components()
        .zip(dst_abs.components())
        .take_while(|(s, d)| s == d)
        .count();

    let src_iter = src_abs.components().skip(suffix_pos).map(|x| x.as_os_str());

    let mut result: PathBuf = dst_abs
        .components()
        .skip(suffix_pos + 1)
        .map(|_| OsStr::new(".."))
        .chain(src_iter)
        .collect();
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    Ok(result.into())
}

fn link(src: &Path, dst: &Path, settings: &Settings) -> Result<()> {
    let mut backup_path = None;
    let source: Cow<'_, Path> = if settings.relative {
        relative_path(src, dst)?
    } else {
        src.into()
    };

    if is_symlink(dst) || dst.exists() {
        match settings.overwrite {
            OverwriteMode::NoClobber => {}
            OverwriteMode::Interactive => {
                print!("{}: overwrite '{}'? ", executable!(), dst.display());
                if !read_yes() {
                    return Ok(());
                }
                fs::remove_file(dst)?
            }
            OverwriteMode::Force => fs::remove_file(dst)?,
        };

        backup_path = match settings.backup {
            BackupMode::NoBackup => None,
            BackupMode::SimpleBackup => Some(simple_backup_path(dst, &settings.suffix)),
            BackupMode::NumberedBackup => Some(numbered_backup_path(dst)),
            BackupMode::ExistingBackup => Some(existing_backup_path(dst, &settings.suffix)),
        };
        if let Some(ref p) = backup_path {
            fs::rename(dst, p)?;
        }
    }

    if settings.symbolic {
        symlink(&source, dst)?;
    } else {
        fs::hard_link(&source, dst)?;
    }

    if settings.verbose {
        print!("'{}' -> '{}'", dst.display(), &source.display());
        match backup_path {
            Some(path) => println!(" (backup: '{}')", path.display()),
            None => println!(),
        }
    }
    Ok(())
}

fn read_yes() -> bool {
    let mut s = String::new();
    match stdin().read_line(&mut s) {
        Ok(_) => match s.char_indices().next() {
            Some((_, x)) => x == 'y' || x == 'Y',
            _ => false,
        },
        _ => false,
    }
}

fn simple_backup_path(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_str().unwrap().to_owned();
    p.push_str(suffix);
    PathBuf::from(p)
}

fn numbered_backup_path(path: &Path) -> PathBuf {
    let mut i: u64 = 1;
    loop {
        let new_path = simple_backup_path(path, &format!(".~{}~", i));
        if !new_path.exists() {
            return new_path;
        }
        i += 1;
    }
}

fn existing_backup_path(path: &Path, suffix: &str) -> PathBuf {
    let test_path = simple_backup_path(path, &".~1~".to_owned());
    if test_path.exists() {
        return numbered_backup_path(path);
    }
    simple_backup_path(path, suffix)
}

#[cfg(windows)]
pub fn symlink<P1: AsRef<Path>, P2: AsRef<Path>>(src: P1, dst: P2) -> Result<()> {
    if src.as_ref().is_dir() {
        symlink_dir(src, dst)
    } else {
        symlink_file(src, dst)
    }
}

pub fn is_symlink<P: AsRef<Path>>(path: P) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) => m.file_type().is_symlink(),
        Err(_) => false,
    }
}
