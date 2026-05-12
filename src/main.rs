use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{Connection, Result, interface};

struct FileChooser {
    terminal: String,
}

impl FileChooser {
    fn new() -> Self {
        Self {
            terminal: std::env::var("XDG_NVFILECHOOSER_TERMINAL")
                .unwrap_or_else(|_| "wezterm".into()),
        }
    }

    fn build_lua_cmd(func: &str, opts_str: &str, tmp_path: &str) -> String {
        if opts_str.is_empty() {
            format!("lua require('xdg-nvfilechooser').{func}({{ output = '{tmp_path}' }})")
        } else {
            format!(
                "lua require('xdg-nvfilechooser').{func}({{ {opts_str}, output = '{tmp_path}' }})"
            )
        }
    }

    fn run_editor(&self, lua_cmd: &str) -> std::result::Result<(), std::io::Error> {
        Command::new(&self.terminal)
            .arg("-e")
            .arg("nvim")
            .arg("-c")
            .arg(lua_cmd)
            .envs(std::env::vars())
            .spawn()?
            .wait()?;
        Ok(())
    }

    fn read_results(path: &Path) -> Vec<String> {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let _ = std::fs::remove_file(path);
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| format!("file://{l}"))
            .collect()
    }

    fn opts_to_string(options: &HashMap<String, Value<'_>>) -> String {
        options
            .iter()
            .map(|(k, v)| value_to_lua(k, v))
            .collect::<Vec<_>>()
            .join(", ")
    }

    async fn handle_open(
        &self,
        options: HashMap<String, Value<'_>>,
    ) -> (u32, HashMap<String, Value<'_>>) {
        let tmp = match tempfile::NamedTempFile::new() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("failed to create temporary file: {e}");
                return (1, HashMap::new());
            }
        };

        let tmp_path = tmp
            .path()
            .to_str()
            .unwrap_or("/tmp/xdg-nvfilechooser-result");

        let opts_str = Self::opts_to_string(&options);
        let lua_cmd = Self::build_lua_cmd("OpenFile", &opts_str, tmp_path);

        if let Err(e) = self.run_editor(&lua_cmd) {
            eprintln!("failed to launch editor: {e}");
            return (1, HashMap::new());
        }

        let uris = Self::read_results(tmp.path());
        if uris.is_empty() {
            return (1, HashMap::new());
        }

        let mut results = HashMap::new();
        results.insert("uris".to_owned(), Value::from(uris));
        (0, results)
    }

    async fn handle_save(
        &self,
        func: &str,
        options: HashMap<String, Value<'_>>,
    ) -> (u32, HashMap<String, Value<'_>>) {
        let tmp = match tempfile::NamedTempFile::new() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("failed to create temporary file: {e}");
                return (1, HashMap::new());
            }
        };
        let tmp_path = tmp
            .path()
            .to_str()
            .unwrap_or("/tmp/xdg-nvfilechooser-result");

        let opts_str = Self::opts_to_string(&options);
        let lua_cmd = Self::build_lua_cmd(func, &opts_str, tmp_path);

        if let Err(e) = self.run_editor(&lua_cmd) {
            eprintln!("failed to launch editor: {e}");
            return (1, HashMap::new());
        }

        let uri = Self::read_results(tmp.path()).into_iter().next();

        match uri {
            None => (1, HashMap::new()),
            Some(uri) => {
                let mut results = HashMap::new();
                results.insert("uris".to_owned(), Value::from(vec![uri]));
                (0, results)
            }
        }
    }
}

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    async fn open_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> (u32, HashMap<String, Value<'_>>) {
        self.handle_open(options).await
    }

    async fn save_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> (u32, HashMap<String, Value<'_>>) {
        self.handle_save("SaveFile", options).await
    }

    async fn save_files(
        &self,
        _handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        options: HashMap<String, Value<'_>>,
    ) -> (u32, HashMap<String, Value<'_>>) {
        self.handle_save("SaveFiles", options).await
    }
}

fn value_to_lua(k: &str, v: &Value) -> String {
    match v {
        Value::Array(a) => {
            let bytes: Vec<u8> = a.iter().filter_map(|v| u8::try_from(v).ok()).collect();
            if !bytes.is_empty() {
                let s = String::from_utf8_lossy(&bytes);
                let s = s.trim_end_matches('\0');
                return format!("{k} = '{s}'");
            }

            let entries: Vec<String> = a
                .iter()
                .filter_map(|entry| {
                    let Value::Structure(s) = entry else {
                        return None;
                    };
                    let fields = s.fields();
                    let Value::Str(name) = &fields[0] else {
                        return None;
                    };
                    let patterns = fields.iter().find_map(|f| match f {
                        Value::Array(arr) => Some(arr),
                        _ => None,
                    })?;
                    let globs = format_globs(patterns);
                    Some(format!("{{ name = '{name}', globs = {{ {globs} }} }}"))
                })
                .collect();

            format!("{k} = {{ {} }}", entries.join(", "))
        }
        Value::Structure(s) => {
            let fields = s.fields();
            let Some(Value::Str(name)) = fields.first() else {
                return format!("{k} = {{}}");
            };
            let patterns = fields.iter().find_map(|f| match f {
                Value::Array(arr) => Some(arr),
                _ => None,
            });
            match patterns {
                Some(pats) => {
                    let globs = format_globs(pats);
                    format!("{k} = {{ name = '{name}', globs = {{ {globs} }} }}")
                }
                None => format!("{k} = {{}}"),
            }
        }
        Value::Bool(b) => format!("{k} = {b}"),
        Value::Str(s) => format!("{k} = '{s}'"),
        Value::U32(n) => format!("{k} = {n}"),
        Value::I32(n) => format!("{k} = {n}"),

        _ => format!("{k} = '{v}'"),
    }
}

fn format_globs(patterns: &[Value]) -> String {
    patterns
        .iter()
        .filter_map(|p| {
            let Value::Structure(pair) = p else {
                return None;
            };
            let fields = pair.fields();
            let Value::U32(kind) = &fields[0] else {
                return None;
            };
            let Value::Str(pattern) = &fields[1] else {
                return None;
            };
            Some(format!("{{ kind = {kind}, pattern = '{pattern}' }}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[tokio::main]
async fn main() -> Result<()> {
    let connection = Connection::session().await?;
    connection
        .request_name("org.freedesktop.impl.portal.desktop.xdg-nvfilechooser")
        .await?;
    connection
        .object_server()
        .at("/org/freedesktop/portal/desktop", FileChooser::new())
        .await?;
    std::future::pending::<()>().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, f64::consts};
    use zbus::zvariant::Structure;

    #[test]
    fn test_value_to_lua_str() {
        let v = Value::from("hello");
        assert_eq!(value_to_lua("key", &v), "key = 'hello'");
    }

    #[test]
    fn test_value_to_lua_bool() {
        assert_eq!(value_to_lua("k", &Value::Bool(true)), "k = true");
        assert_eq!(value_to_lua("k", &Value::Bool(false)), "k = false");
    }

    #[test]
    fn test_value_to_lua_u32() {
        assert_eq!(value_to_lua("k", &Value::U32(42)), "k = 42");
    }

    #[test]
    fn test_value_to_lua_i32() {
        assert_eq!(value_to_lua("k", &Value::I32(-7)), "k = -7");
    }

    #[test]
    fn test_value_to_lua_fallback() {
        let v = Value::F64(consts::PI);
        assert_eq!(value_to_lua("k", &v), "k = '3.14'");
    }

    #[test]
    fn test_value_to_lua_byte_array() {
        let v = Value::from(vec![72u8, 105u8]);
        assert_eq!(value_to_lua("k", &v), "k = 'Hi'");
    }

    #[test]
    fn test_value_to_lua_byte_array_with_nul() {
        let v = Value::from(vec![104u8, 101u8, 108u8, 108u8, 111u8, 0u8]);
        assert_eq!(value_to_lua("k", &v), "k = 'hello'");
    }

    #[test]
    fn test_value_to_lua_empty_array() {
        let v: Value<'_> = Value::from(Vec::<u8>::new());
        assert_eq!(value_to_lua("k", &v), "k = {  }");
    }

    #[test]
    fn test_value_to_lua_filter_array() {
        let filters: Value<'_> = vec![("myfilter".to_string(), vec![(0u32, "*.txt")])].into();
        assert_eq!(
            value_to_lua("filters", &filters),
            "filters = { { name = 'myfilter', globs = { { kind = 0, pattern = '*.txt' } } } }"
        );
    }

    #[test]
    fn test_value_to_lua_filter_array_multiple() {
        let filters: Value<'_> =
            vec![("docs".to_string(), vec![(0u32, "*.md"), (0u32, "*.txt")])].into();
        assert_eq!(
            value_to_lua("filters", &filters),
            "filters = { { name = 'docs', globs = { { kind = 0, pattern = '*.md' }, { kind = 0, pattern = '*.txt' } } } }"
        );
    }

    #[test]
    fn test_value_to_lua_filter_array_multiple_filters() {
        let filters: Value<'_> = vec![
            ("images".to_string(), vec![(0u32, "*.png")]),
            ("text".to_string(), vec![(0u32, "*.txt")]),
        ]
        .into();
        assert_eq!(
            value_to_lua("filters", &filters),
            "filters = { { name = 'images', globs = { { kind = 0, pattern = '*.png' } } }, { name = 'text', globs = { { kind = 0, pattern = '*.txt' } } } }"
        );
    }

    #[test]
    fn test_value_to_lua_structure_single() {
        let v = Value::Structure(Structure::from(("mystr".to_string(),)));
        assert_eq!(value_to_lua("k", &v), "k = {}");
    }

    #[test]
    fn test_value_to_lua_structure_with_globs() {
        let v: Value<'_> = ("code".to_string(), vec![(1u32, "*.rs")]).into();
        assert_eq!(
            value_to_lua("k", &v),
            "k = { name = 'code', globs = { { kind = 1, pattern = '*.rs' } } }"
        );
    }

    #[test]
    fn test_values_to_lua_untagged_structure() {
        let s = Structure::from((Value::U32(42), Value::Bool(true)));
        let v = Value::Structure(s);
        assert_eq!(value_to_lua("k", &v), "k = {}");
    }

    #[test]
    fn test_format_globs_single() {
        let p1 = Value::from((0u32, "*.txt"));
        let patterns = [p1];
        assert_eq!(format_globs(&patterns), "{ kind = 0, pattern = '*.txt' }");
    }

    #[test]
    fn test_format_globs_multiple() {
        let p1 = Value::from((0u32, "*.txt"));
        let p2 = Value::from((1u32, "*.rs"));
        let patterns = [p1, p2];
        assert_eq!(
            format_globs(&patterns),
            "{ kind = 0, pattern = '*.txt' }, { kind = 1, pattern = '*.rs' }"
        );
    }

    #[test]
    fn test_format_globs_empty() {
        let patterns: [Value<'_>; 0] = [];
        assert_eq!(format_globs(&patterns), "");
    }

    #[test]
    fn test_format_globs_invalid_entry() {
        let p1 = Value::from("not_a_structure");
        let patterns = [p1];
        assert_eq!(format_globs(&patterns), "");
    }

    #[test]
    fn test_build_lua_cmd_with_opts() {
        let cmd = FileChooser::build_lua_cmd("OpenFile", "filter = '*.txt'", "/tmp/test");
        assert_eq!(
            cmd,
            "lua require('xdg-nvfilechooser').OpenFile({ filter = '*.txt', output = '/tmp/test' })"
        );
    }

    #[test]
    fn test_build_lua_cmd_no_opts() {
        let cmd = FileChooser::build_lua_cmd("SaveFile", "", "/tmp/test");
        assert_eq!(
            cmd,
            "lua require('xdg-nvfilechooser').SaveFile({ output = '/tmp/test' })"
        );
    }

    #[test]
    fn test_build_lua_cmd_save_files() {
        let cmd = FileChooser::build_lua_cmd("SaveFiles", "current_name = 'foo.txt'", "/tmp/test");
        assert_eq!(
            cmd,
            "lua require('xdg-nvfilechooser').SaveFiles({ current_name = 'foo.txt', output = '/tmp/test' })"
        );
    }

    #[test]
    fn test_opts_to_string() {
        let mut opts = HashMap::new();
        opts.insert("current_name".to_string(), Value::from("hello.txt"));
        opts.insert("directory".to_string(), Value::from("/home/user"));
        let result = FileChooser::opts_to_string(&opts);
        assert!(result.contains("current_name = 'hello.txt'"));
        assert!(result.contains("directory = '/home/user'"));
    }

    #[test]
    fn test_opts_to_string_empty() {
        let opts = HashMap::new();
        assert_eq!(FileChooser::opts_to_string(&opts), "");
    }

    #[test]
    fn test_opts_to_string_mixed() {
        let mut opts = HashMap::new();
        opts.insert("multiple".to_string(), Value::Bool(true));
        opts.insert("limit".to_string(), Value::U32(5));
        let result = FileChooser::opts_to_string(&opts);
        assert!(result.contains("multiple = true"));
        assert!(result.contains("limit = 5"));
    }

    #[test]
    fn test_read_results_returns_uris() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("results");
        std::fs::write(&path, "/home/user/doc.txt\n/home/user/doc2.txt\n").unwrap();

        let uris = FileChooser::read_results(&path);
        assert_eq!(
            uris,
            vec!["file:///home/user/doc.txt", "file:///home/user/doc2.txt"]
        );
        assert!(!path.exists());
    }

    #[test]
    fn test_read_results_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, "").unwrap();

        let uris = FileChooser::read_results(&path);
        assert!(uris.is_empty());
    }

    #[test]
    fn test_read_results_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blanks");
        std::fs::write(&path, "\n\n/file.txt\n\n").unwrap();

        let uris = FileChooser::read_results(&path);
        assert_eq!(uris, vec!["file:///file.txt"]);
    }

    #[test]
    fn test_read_results_nonexistent() {
        let path = Path::new("/tmp/__nonexistent_xdg_test__");
        let _ = std::fs::remove_file(path);
        let uris = FileChooser::read_results(path);
        assert!(uris.is_empty());
    }

    #[test]
    fn test_new_uses_defaults() {
        let fc = FileChooser::new();
        assert_eq!(fc.terminal, "wezterm");
    }

    #[test]
    fn test_new_respects_env() {
        let prev = std::env::var("XDG_NVFILECHOOSER_TERMINAL").ok();

        unsafe {
            std::env::set_var("XDG_NVFILECHOOSER_TERMINAL", "foot");
        }

        let fc = FileChooser::new();
        assert_eq!(fc.terminal, "foot");

        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_NVFILECHOOSER_TERMINAL", v),
                None => std::env::remove_var("XDG_NVFILECHOOSER_TERMINAL"),
            }
        }
    }
}
