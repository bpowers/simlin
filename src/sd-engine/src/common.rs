use std;
use std::convert::From;
use std::fmt;
use std::{error, result};

use sd::core;

pub type Ident = String;

// from https://stackoverflow.com/questions/27588416/how-to-send-output-to-stderr
#[macro_export]
macro_rules! eprintln(
    ($($arg:tt)*) => { {
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

#[macro_export]
macro_rules! die(
    ($($arg:tt)*) => { {
        use std;
        eprintln!($($arg)*);
        std::process::exit(1/*EXIT_FAILURE*/)
    } }
);
#[macro_export]
macro_rules! err(
    ($($arg:tt)*) => { {
        use sd::common::SDError;
        Err(SDError::new(format!($($arg)*)))
    } }
);

#[derive(Debug)]
pub struct SDError {
    msg: String,
}

impl SDError {
    pub fn new(msg: String) -> SDError {
        SDError { msg: msg }
    }
}

impl fmt::Display for SDError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl error::Error for SDError {
    fn description(&self) -> &str {
        &self.msg
    }
}

impl From<std::io::Error> for SDError {
    fn from(err: std::io::Error) -> Self {
        SDError {
            msg: format!("io::Error: {:?}", err),
        }
    }
}

impl From<core::num::ParseFloatError> for SDError {
    fn from(err: core::num::ParseFloatError) -> Self {
        SDError {
            msg: format!("ParseFloatError: {:?}", err),
        }
    }
}

pub type Result<T> = result::Result<T, SDError>;

pub fn canonicalize(name: &str) -> String {
    // remove leading and trailing whitespace, do this before testing
    // for quotedness as we should treat a quoted string as sacrosanct
    let name = name.trim();

    let bytes = name.as_bytes();
    let quoted: bool =
        { bytes.len() >= 2 && bytes[0] == '"' as u8 && bytes[bytes.len() - 1] == '"' as u8 };

    let name = if quoted {
        &name[1..bytes.len() - 1]
    } else {
        name
    };

    lazy_static! {
        static ref BACKSLASH_RE: Regex = Regex::new(r"\\\\").unwrap();
        // TODO: \x{C2AO} ?
        static ref UNDERSCORE_RE: Regex = Regex::new(r"\\n|\\r|\n|\r| |\x{00A0}").unwrap();
    }
    let name = BACKSLASH_RE.replace_all(name, "\\");
    let name = UNDERSCORE_RE.replace_all(&name, "_");

    name.to_lowercase()
}

#[test]
fn test_canonicalize() {
    assert!(canonicalize("\"quoted\"") == "quoted");
    assert!(canonicalize("   a b") == "a_b");
    assert!(canonicalize("Å\nb") == "å_b");
}
