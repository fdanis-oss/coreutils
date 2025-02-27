//! All utils return exit with an exit code. Usually, the following scheme is used:
//! * `0`: succeeded
//! * `1`: minor problems
//! * `2`: major problems
//!
//! This module provides types to reconcile these exit codes with idiomatic Rust error
//! handling. This has a couple advantages over manually using [`std::process::exit`]:
//! 1. It enables the use of `?`, `map_err`, `unwrap_or`, etc. in `uumain`.
//! 1. It encourages the use of `UResult`/`Result` in functions in the utils.
//! 1. The error messages are largely standardized across utils.
//! 1. Standardized error messages can be created from external result types
//!    (i.e. [`std::io::Result`] & `clap::ClapResult`).
//! 1. `set_exit_code` takes away the burden of manually tracking exit codes for non-fatal errors.
//!
//! # Usage
//! The signature of a typical util should be:
//! ```ignore
//! fn uumain(args: impl uucore::Args) -> UResult<()> {
//!     ...
//! }
//! ```
//! [`UResult`] is a simple wrapper around [`Result`] with a custom error type: [`UError`]. The
//! most important difference with types implementing [`std::error::Error`] is that [`UError`]s
//! can specify the exit code of the program when they are returned from `uumain`:
//! * When `Ok` is returned, the code set with [`set_exit_code`] is used as exit code. If
//!   [`set_exit_code`] was not used, then `0` is used.
//! * When `Err` is returned, the code corresponding with the error is used as exit code and the
//! error message is displayed.
//!
//! Additionally, the errors can be displayed manually with the [`show`] and [`show_if_err`] macros:
//! ```ignore
//! let res = Err(USimpleError::new(1, "Error!!"));
//! show_if_err!(res);
//! // or
//! if let Err(e) = res {
//!    show!(e);
//! }
//! ```
//!
//! **Note**: The [`show`] and [`show_if_err`] macros set the exit code of the program using
//! [`set_exit_code`]. See the documentation on that function for more information.
//!
//! # Guidelines
//! * Use common errors where possible.
//! * Add variants to [`UCommonError`] if an error appears in multiple utils.
//! * Prefer proper custom error types over [`ExitCode`] and [`USimpleError`].
//! * [`USimpleError`] may be used in small utils with simple error handling.
//! * Using [`ExitCode`] is not recommended but can be useful for converting utils to use
//!   [`UResult`].

// spell-checker:ignore uioerror

use std::{
    error::Error,
    fmt::{Display, Formatter},
    sync::atomic::{AtomicI32, Ordering},
};

static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// Get the last exit code set with [`set_exit_code`].
/// The default value is `0`.
pub fn get_exit_code() -> i32 {
    EXIT_CODE.load(Ordering::SeqCst)
}

/// Set the exit code for the program if `uumain` returns `Ok(())`.
///
/// This function is most useful for non-fatal errors, for example when applying an operation to
/// multiple files:
/// ```ignore
/// use uucore::error::{UResult, set_exit_code};
///
/// fn uumain(args: impl uucore::Args) -> UResult<()> {
///     ...
///     for file in files {
///         let res = some_operation_that_might_fail(file);
///         match res {
///             Ok() => {},
///             Err(_) => set_exit_code(1),
///         }
///     }
///     Ok(()) // If any of the operations failed, 1 is returned.
/// }
/// ```
pub fn set_exit_code(code: i32) {
    EXIT_CODE.store(code, Ordering::SeqCst);
}

/// Should be returned by all utils.
///
/// Two additional methods are implemented on [`UResult`] on top of the normal [`Result`] methods:
/// `map_err_code` & `map_err_code_message`.
///
/// These methods are used to convert [`UCommonError`]s into errors with a custom error code and
/// message.
pub type UResult<T> = Result<T, UError>;

trait UResultTrait<T> {
    fn map_err_code(self, mapper: fn(&UCommonError) -> Option<i32>) -> Self;
    fn map_err_code_and_message(self, mapper: fn(&UCommonError) -> Option<(i32, String)>) -> Self;
}

impl<T> UResultTrait<T> for UResult<T> {
    fn map_err_code(self, mapper: fn(&UCommonError) -> Option<i32>) -> Self {
        if let Err(UError::Common(error)) = self {
            if let Some(code) = mapper(&error) {
                Err(UCommonErrorWithCode { code, error }.into())
            } else {
                Err(error.into())
            }
        } else {
            self
        }
    }

    fn map_err_code_and_message(self, mapper: fn(&UCommonError) -> Option<(i32, String)>) -> Self {
        if let Err(UError::Common(ref error)) = self {
            if let Some((code, message)) = mapper(error) {
                return Err(USimpleError { code, message }.into());
            }
        }
        self
    }
}

/// The error type of [`UResult`].
///
/// `UError::Common` errors are defined in [`uucore`](crate) while `UError::Custom` errors are
/// defined by the utils.
/// ```
/// use uucore::error::USimpleError;
/// let err = USimpleError::new(1, "Error!!".into());
/// assert_eq!(1, err.code());
/// assert_eq!(String::from("Error!!"), format!("{}", err));
/// ```
pub enum UError {
    Common(UCommonError),
    Custom(Box<dyn UCustomError>),
}

impl UError {
    /// The error code of [`UResult`]
    ///
    /// This function defines the error code associated with an instance of
    /// [`UResult`]. To associate error codes for self-defined instances of
    /// `UResult::Custom` (i.e. [`UCustomError`]), implement the
    /// [`code`-function there](UCustomError::code).
    pub fn code(&self) -> i32 {
        match self {
            UError::Common(e) => e.code(),
            UError::Custom(e) => e.code(),
        }
    }

    /// Whether to print usage help for a [`UResult`]
    ///
    /// Defines if a variant of [`UResult`] should print a short usage message
    /// below the error. The usage message is printed when this function returns
    /// `true`. To do this for self-defined instances of `UResult::Custom` (i.e.
    /// [`UCustomError`]), implement the [`usage`-function
    /// there](UCustomError::usage).
    pub fn usage(&self) -> bool {
        match self {
            UError::Common(e) => e.usage(),
            UError::Custom(e) => e.usage(),
        }
    }
}

impl From<UCommonError> for UError {
    fn from(v: UCommonError) -> Self {
        UError::Common(v)
    }
}

impl From<i32> for UError {
    fn from(v: i32) -> Self {
        UError::Custom(Box::new(ExitCode(v)))
    }
}

impl<E: UCustomError + 'static> From<E> for UError {
    fn from(v: E) -> Self {
        UError::Custom(Box::new(v) as Box<dyn UCustomError>)
    }
}

impl Display for UError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UError::Common(e) => e.fmt(f),
            UError::Custom(e) => e.fmt(f),
        }
    }
}

/// Custom errors defined by the utils.
///
/// All errors should implement [`std::error::Error`], [`std::fmt::Display`] and
/// [`std::fmt::Debug`] and have an additional `code` method that specifies the
/// exit code of the program if the error is returned from `uumain`.
///
/// An example of a custom error from `ls`:
///
/// ```
/// use uucore::error::{UCustomError, UResult};
/// use std::{
///     error::Error,
///     fmt::{Display, Debug},
///     path::PathBuf
/// };
///
/// #[derive(Debug)]
/// enum LsError {
///     InvalidLineWidth(String),
///     NoMetadata(PathBuf),
/// }
///
/// impl UCustomError for LsError {
///     fn code(&self) -> i32 {
///         match self {
///             LsError::InvalidLineWidth(_) => 2,
///             LsError::NoMetadata(_) => 1,
///         }
///     }
/// }
///
/// impl Error for LsError {}
///
/// impl Display for LsError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         match self {
///             LsError::InvalidLineWidth(s) => write!(f, "invalid line width: '{}'", s),
///             LsError::NoMetadata(p) => write!(f, "could not open file: '{}'", p.display()),
///         }
///     }
/// }
/// ```
///
/// The main routine would look like this:
///
/// ```ignore
/// #[uucore_procs::gen_uumain]
/// pub fn uumain(args: impl uucore::Args) -> UResult<()> {
///     // Perform computations here ...
///     return Err(LsError::InvalidLineWidth(String::from("test")).into())
/// }
/// ```
///
/// The call to `into()` is required to convert the [`UCustomError`] to an
/// instance of [`UError`].
///
/// A crate like [`quick_error`](https://crates.io/crates/quick-error) might
/// also be used, but will still require an `impl` for the `code` method.
pub trait UCustomError: Error + Send {
    /// Error code of a custom error.
    ///
    /// Set a return value for each variant of an enum-type to associate an
    /// error code (which is returned to the system shell) with an error
    /// variant.
    ///
    /// # Example
    ///
    /// ```
    /// use uucore::error::{UCustomError};
    /// use std::{
    ///     error::Error,
    ///     fmt::{Display, Debug},
    ///     path::PathBuf
    /// };
    ///
    /// #[derive(Debug)]
    /// enum MyError {
    ///     Foo(String),
    ///     Bar(PathBuf),
    ///     Bing(),
    /// }
    ///
    /// impl UCustomError for MyError {
    ///     fn code(&self) -> i32 {
    ///         match self {
    ///             MyError::Foo(_) => 2,
    ///             // All other errors yield the same error code, there's no
    ///             // need to list them explicitly.
    ///             _ => 1,
    ///         }
    ///     }
    /// }
    ///
    /// impl Error for MyError {}
    ///
    /// impl Display for MyError {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         use MyError as ME;
    ///         match self {
    ///             ME::Foo(s) => write!(f, "Unknown Foo: '{}'", s),
    ///             ME::Bar(p) => write!(f, "Couldn't find Bar: '{}'", p.display()),
    ///             ME::Bing() => write!(f, "Exterminate!"),
    ///         }
    ///     }
    /// }
    /// ```
    fn code(&self) -> i32 {
        1
    }

    /// Print usage help to a custom error.
    ///
    /// Return true or false to control whether a short usage help is printed
    /// below the error message. The usage help is in the format: "Try '{name}
    /// --help' for more information." and printed only if `true` is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use uucore::error::{UCustomError};
    /// use std::{
    ///     error::Error,
    ///     fmt::{Display, Debug},
    ///     path::PathBuf
    /// };
    ///
    /// #[derive(Debug)]
    /// enum MyError {
    ///     Foo(String),
    ///     Bar(PathBuf),
    ///     Bing(),
    /// }
    ///
    /// impl UCustomError for MyError {
    ///     fn usage(&self) -> bool {
    ///         match self {
    ///             // This will have a short usage help appended
    ///             MyError::Bar(_) => true,
    ///             // These matches won't have a short usage help appended
    ///             _ => false,
    ///         }
    ///     }
    /// }
    ///
    /// impl Error for MyError {}
    ///
    /// impl Display for MyError {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         use MyError as ME;
    ///         match self {
    ///             ME::Foo(s) => write!(f, "Unknown Foo: '{}'", s),
    ///             ME::Bar(p) => write!(f, "Couldn't find Bar: '{}'", p.display()),
    ///             ME::Bing() => write!(f, "Exterminate!"),
    ///         }
    ///     }
    /// }
    /// ```
    fn usage(&self) -> bool {
        false
    }
}

impl From<Box<dyn UCustomError>> for i32 {
    fn from(e: Box<dyn UCustomError>) -> i32 {
        e.code()
    }
}

/// A [`UCommonError`] with an overridden exit code.
///
/// This exit code is returned instead of the default exit code for the [`UCommonError`]. This is
/// typically created with the either the `UResult::map_err_code` or `UCommonError::with_code`
/// method.
#[derive(Debug)]
pub struct UCommonErrorWithCode {
    code: i32,
    error: UCommonError,
}

impl Error for UCommonErrorWithCode {}

impl Display for UCommonErrorWithCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.error.fmt(f)
    }
}

impl UCustomError for UCommonErrorWithCode {
    fn code(&self) -> i32 {
        self.code
    }
}

/// A simple error type with an exit code and a message that implements [`UCustomError`].
///
/// It is typically created with the `UResult::map_err_code_and_message` method. Alternatively, it
/// can be constructed by manually:
/// ```
/// use uucore::error::{UResult, USimpleError};
/// let err = USimpleError { code: 1, message: "error!".into()};
/// let res: UResult<()> = Err(err.into());
/// // or using the `new` method:
/// let res: UResult<()> = Err(USimpleError::new(1, "error!".into()));
/// ```
#[derive(Debug)]
pub struct USimpleError {
    pub code: i32,
    pub message: String,
}

impl USimpleError {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(code: i32, message: String) -> UError {
        UError::Custom(Box::new(Self { code, message }))
    }
}

impl Error for USimpleError {}

impl Display for USimpleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.message.fmt(f)
    }
}

impl UCustomError for USimpleError {
    fn code(&self) -> i32 {
        self.code
    }
}

#[derive(Debug)]
pub struct UUsageError {
    pub code: i32,
    pub message: String,
}

impl UUsageError {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(code: i32, message: String) -> UError {
        UError::Custom(Box::new(Self { code, message }))
    }
}

impl Error for UUsageError {}

impl Display for UUsageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.message.fmt(f)
    }
}

impl UCustomError for UUsageError {
    fn code(&self) -> i32 {
        self.code
    }

    fn usage(&self) -> bool {
        true
    }
}

/// Wrapper type around [`std::io::Error`].
///
/// The messages displayed by [`UIoError`] should match the error messages displayed by GNU
/// coreutils.
///
/// There are two ways to construct this type: with [`UIoError::new`] or by calling the
/// [`FromIo::map_err_context`] method on a [`std::io::Result`] or [`std::io::Error`].
/// ```
/// use uucore::error::{FromIo, UResult, UIoError, UCommonError};
/// use std::fs::File;
/// use std::path::Path;
/// let path = Path::new("test.txt");
///
/// // Manual construction
/// let e: UIoError = UIoError::new(
///     std::io::ErrorKind::NotFound,
///     format!("cannot access '{}'", path.display())
/// );
/// let res: UResult<()> = Err(e.into());
///
/// // Converting from an `std::io::Error`.
/// let res: UResult<File> = File::open(path).map_err_context(|| format!("cannot access '{}'", path.display()));
/// ```
#[derive(Debug)]
pub struct UIoError {
    context: String,
    inner: std::io::Error,
}

impl UIoError {
    pub fn new(kind: std::io::ErrorKind, context: String) -> Self {
        Self {
            context,
            inner: std::io::Error::new(kind, ""),
        }
    }

    pub fn code(&self) -> i32 {
        1
    }

    pub fn usage(&self) -> bool {
        false
    }
}

impl Error for UIoError {}

impl Display for UIoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        use std::io::ErrorKind::*;
        write!(
            f,
            "{}: {}",
            self.context,
            match self.inner.kind() {
                NotFound => "No such file or directory",
                PermissionDenied => "Permission denied",
                ConnectionRefused => "Connection refused",
                ConnectionReset => "Connection reset",
                ConnectionAborted => "Connection aborted",
                NotConnected => "Not connected",
                AddrInUse => "Address in use",
                AddrNotAvailable => "Address not available",
                BrokenPipe => "Broken pipe",
                AlreadyExists => "Already exists",
                WouldBlock => "Would block",
                InvalidInput => "Invalid input",
                InvalidData => "Invalid data",
                TimedOut => "Timed out",
                WriteZero => "Write zero",
                Interrupted => "Interrupted",
                Other => "Other",
                UnexpectedEof => "Unexpected end of file",
                _ => panic!("Unexpected io error: {}", self.inner),
            },
        )
    }
}

/// Enables the conversion from `std::io::Error` to `UError` and from `std::io::Result` to
/// `UResult`.
pub trait FromIo<T> {
    fn map_err_context(self, context: impl FnOnce() -> String) -> T;
}

impl FromIo<UIoError> for std::io::Error {
    fn map_err_context(self, context: impl FnOnce() -> String) -> UIoError {
        UIoError {
            context: (context)(),
            inner: self,
        }
    }
}

impl<T> FromIo<UResult<T>> for std::io::Result<T> {
    fn map_err_context(self, context: impl FnOnce() -> String) -> UResult<T> {
        self.map_err(|e| UError::Common(UCommonError::Io(e.map_err_context(context))))
    }
}

impl FromIo<UIoError> for std::io::ErrorKind {
    fn map_err_context(self, context: impl FnOnce() -> String) -> UIoError {
        UIoError {
            context: (context)(),
            inner: std::io::Error::new(self, ""),
        }
    }
}

impl From<UIoError> for UCommonError {
    fn from(e: UIoError) -> UCommonError {
        UCommonError::Io(e)
    }
}

impl From<UIoError> for UError {
    fn from(e: UIoError) -> UError {
        let common: UCommonError = e.into();
        common.into()
    }
}

/// Shorthand to construct [`UIoError`]-instances.
///
/// This macro serves as a convenience call to quickly construct instances of
/// [`UIoError`]. It takes:
///
/// - An instance of [`std::io::Error`]
/// - A `format!`-compatible string and
/// - An arbitrary number of arguments to the format string
///
/// In exactly this order. It is equivalent to the more verbose code seen in the
/// example.
///
/// # Examples
///
/// ```
/// use uucore::error::UIoError;
/// use uucore::uio_error;
///
/// let io_err = std::io::Error::new(
///     std::io::ErrorKind::PermissionDenied, "fix me please!"
/// );
///
/// let uio_err = UIoError::new(
///     io_err.kind(),
///     format!("Error code: {}", 2)
/// );
///
/// let other_uio_err = uio_error!(io_err, "Error code: {}", 2);
///
/// // prints "fix me please!: Permission denied"
/// println!("{}", uio_err);
/// // prints "Error code: 2: Permission denied"
/// println!("{}", other_uio_err);
/// ```
///
/// The [`std::fmt::Display`] impl of [`UIoError`] will then ensure that an
/// appropriate error message relating to the actual error kind of the
/// [`std::io::Error`] is appended to whatever error message is defined in
/// addition (as secondary argument).
///
/// If you want to show only the error message for the [`std::io::ErrorKind`]
/// that's contained in [`UIoError`], pass the second argument as empty string:
///
/// ```
/// use uucore::error::UIoError;
/// use uucore::uio_error;
///
/// let io_err = std::io::Error::new(
///     std::io::ErrorKind::PermissionDenied, "fix me please!"
/// );
///
/// let other_uio_err = uio_error!(io_err, "");
///
/// // prints: ": Permission denied"
/// println!("{}", other_uio_err);
/// ```
//#[macro_use]
#[macro_export]
macro_rules! uio_error(
    ($err:expr, $($args:tt)+) => ({
        UIoError::new(
            $err.kind(),
            format!($($args)+)
        )
    })
);

/// Common errors for utilities.
///
/// If identical errors appear across multiple utilities, they should be added here.
#[derive(Debug)]
pub enum UCommonError {
    Io(UIoError),
    // Clap(UClapError),
}

impl UCommonError {
    pub fn with_code(self, code: i32) -> UCommonErrorWithCode {
        UCommonErrorWithCode { code, error: self }
    }

    pub fn code(&self) -> i32 {
        1
    }

    pub fn usage(&self) -> bool {
        false
    }
}

impl From<UCommonError> for i32 {
    fn from(common: UCommonError) -> i32 {
        match common {
            UCommonError::Io(e) => e.code(),
        }
    }
}

impl Error for UCommonError {}

impl Display for UCommonError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            UCommonError::Io(e) => e.fmt(f),
        }
    }
}

/// A special error type that does not print any message when returned from
/// `uumain`. Especially useful for porting utilities to using [`UResult`].
///
/// There are two ways to construct an [`ExitCode`]:
/// ```
/// use uucore::error::{ExitCode, UResult};
/// // Explicit
/// let res: UResult<()> = Err(ExitCode(1).into());
///
/// // Using into on `i32`:
/// let res: UResult<()> = Err(1.into());
/// ```
/// This type is especially useful for a trivial conversion from utils returning [`i32`] to
/// returning [`UResult`].  
#[derive(Debug)]
pub struct ExitCode(pub i32);

impl Error for ExitCode {}

impl Display for ExitCode {
    fn fmt(&self, _: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        Ok(())
    }
}

impl UCustomError for ExitCode {
    fn code(&self) -> i32 {
        self.0
    }
}
