// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//

use std::{
    env,
    path::{Path, PathBuf},
};

/// Determine the path to a log configuration file using the following precedence rules:
/// 1. Use the provided path (usually pulled from a CLI argument)
/// 2. Use the value in the `TARI_LOG_CONFIGURATION` envar
/// 3. The default path (OS-dependent), "~/.tari/log4rs.toml`
/// 4. The current directory
pub fn get_log_configuration_path(cli_path: Option<PathBuf>) -> PathBuf {
    cli_path
        .or_else(|| {
            env::var_os("TARI_LOG_CONFIGURATION")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| dirs::home_dir().map(|path| path.join(".tari/log4rs.yml")))
        .or_else(|| {
            Some(env::current_dir().expect(
                "Could find a suitable path to the log configuration file. Consider setting the \
                 TARI_LOG_CONFIGURATION envar, or check that the current directory exists and that you have \
                 permission to read it",
            ))
        })
        .unwrap()
}

/// Set up application-level logging using the Log4rs configuration file specified in
pub fn initialize_logging(config_file: &Path) -> bool {
    println!(
        "Initializing logging according to {:?}",
        config_file.to_str().unwrap_or("[??]")
    );
    if let Err(e) = log4rs::init_file(config_file.clone(), Default::default()) {
        println!("We couldn't load a logging configuration file. {}", e.to_string());
        return false;
    }
    true
}

/// Installs a new default logfile configuration, copied from `log4rs-sample.yml` to the given path.
/// When bundled as a binary, the config sample file must be bundled in `common/config`.
pub fn install_default_logfile_config(path: &Path) -> Result<u64, std::io::Error> {
    let mut source = env::current_dir()?;
    source.push(Path::new("common/logging/log4rs-sample.yml"));
    std::fs::copy(source, path)
}

/// Log an error if an `Err` is returned from the `$expr`. If the given expression is `Ok(v)`,
/// `Some(v)` is returned, otherwise `None` is returned (same as `Result::ok`).
/// Useful in cases where the error should be logged and ignored.
/// instead of writing `if let Err(err) = my_error_call() { error!(...) }`, you can write
/// `log_if_error!(my_error_call())`
///
/// ```edition2018
/// # use tari_common::log_if_error;
/// let opt = log_if_error!(level: debug, target: "docs", "Error sending reply: {}", Result::<(), _>::Err("this will be logged"));
/// assert_eq!(opt, None);
/// ```
#[macro_export]
macro_rules! log_if_error {
    (level:$level:tt, target: $target:expr, $msg:expr, $expr:expr $(,)*) => {{
        match $expr {
            Ok(v) => Some(v),
            Err(err) => {
                log::$level!(target: $target, $msg, err);
                None
            }
        }
    }};
    (level:$level:tt, $msg:expr, $expr:expr $(,)*) => {{
        log_if_error!(level:$level, target: "$crate", $msg, $expr)
    }};
     (target: $target:expr, $msg:expr, $expr:expr $(,)*) => {{
        log_if_error!(level:error, target: $target, $msg, $expr)
    }};
    ($msg:expr, $expr:expr $(,)*) => {{
        log_if_error!(level:error, target: "$crate", $msg, $expr)
    }};
}

/// See [log_if_error!](./log_if_error.macro.html).
///
/// ```edition2018
/// # use tari_common::log_if_error_fmt;
/// let opt = log_if_error_fmt!(level: debug, target: "docs", "Error sending reply - custom: {}", Result::<(), _>::Err(()), "this is logged");
/// assert_eq!(opt, None);
/// ```
#[macro_export]
macro_rules! log_if_error_fmt {
    (level: $level:tt, target: $target:expr, $msg:expr, $expr:expr, $($args:tt)+) => {{
        match $expr {
            Ok(v) => Some(v),
            Err(_) => {
                log::$level!(target: $target, $msg, $($args)+);
                None
            }
        }
    }};
}

#[cfg(test)]
mod test {
    use crate::logging::get_log_configuration_path;
    use std::{env, path::PathBuf};

    #[test]
    fn get_log_configuration_path_cli() {
        let path = get_log_configuration_path(Some(PathBuf::from("~/my-tari")));
        assert_eq!(path.to_str().unwrap(), "~/my-tari");
    }

    #[test]
    fn get_log_configuration_path_by_env_var() {
        env::set_var("TARI_LOG_CONFIGURATION", "~/fake-example");
        let path = get_log_configuration_path(None);
        assert_eq!(path.to_str().unwrap(), "~/fake-example");
        env::set_var("TARI_LOG_CONFIGURATION", "");
    }

    #[test]
    fn log_if_error() {
        let err = Result::<(), _>::Err("What a shame");
        let opt = log_if_error!("Error: {}", err);
        assert!(opt.is_none());

        let opt = log_if_error!(level: trace, "Error: {}", err);
        assert!(opt.is_none());

        let opt = log_if_error!(level: trace, "Error: {}", Result::<_, &str>::Ok("answer"));
        assert_eq!(opt, Some("answer"));
    }
}
