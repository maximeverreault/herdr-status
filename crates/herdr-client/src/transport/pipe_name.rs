//! Windows named-pipe name derivation. Platform independent so it is unit
//! tested on the Mac too.

/// herdr opens its socket "path" through the `interprocess` crate's
/// `GenericNamespaced` mapping, which prepends this prefix to the path string
/// verbatim (backslashes and drive letter included).
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

pub fn pipe_name_for(socket_path: &str) -> String {
    format!("{PIPE_PREFIX}{socket_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepends_pipe_prefix_verbatim() {
        assert_eq!(
            pipe_name_for(r"C:\Users\me\AppData\Roaming\herdr\herdr.sock"),
            r"\\.\pipe\C:\Users\me\AppData\Roaming\herdr\herdr.sock"
        );
    }
}
