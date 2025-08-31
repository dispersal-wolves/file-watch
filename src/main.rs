mod sha256;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    size: u64,
    modified_ns: u128,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Change {
    path: String,
    kind: ChangeKind,
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(sha256::hex(&bytes))
}

fn normalized_relative(root: &Path, path: &Path) -> io::Result<String> {
    path.strip_prefix(root)
        .map(|value| value.to_string_lossy().replace('\\', "/"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path escaped root"))
}

fn matches_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return path.starts_with(prefix);
    }
    path == pattern || path.split('/').any(|part| part == pattern)
}

fn snapshot(root: &Path, excludes: &[String]) -> io::Result<BTreeMap<String, Entry>> {
    let root = root.canonicalize()?;
    let mut output = BTreeMap::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        for item in fs::read_dir(directory)? {
            let item = item?;
            let path = item.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let relative = normalized_relative(&root, &path)?;
            if excludes
                .iter()
                .any(|pattern| matches_pattern(&relative, pattern))
            {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let modified_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_nanos())
                    .unwrap_or(0);
                output.insert(
                    relative,
                    Entry {
                        size: metadata.len(),
                        modified_ns,
                        sha256: hash_file(&path)?,
                    },
                );
            }
        }
    }
    Ok(output)
}

fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|byte| match byte {
            b'%' | b'\t' | b'\n' | b'\r' => format!("%{byte:02X}"),
            _ => (byte as char).to_string(),
        })
        .collect()
}

fn decode_path(path: &str) -> io::Result<String> {
    let mut bytes = Vec::new();
    let input = path.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if index + 2 >= input.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad path escape",
                ));
            }
            let value = u8::from_str_radix(&path[index + 1..index + 3], 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad path escape"))?;
            bytes.push(value);
            index += 3;
        } else {
            bytes.push(input[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path is not UTF-8"))
}

fn save(path: &Path, entries: &BTreeMap<String, Entry>) -> io::Result<()> {
    let mut text = String::from("# dispersal-wolves/file-watch/v1\n");
    for (name, entry) in entries {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            encode_path(name),
            entry.size,
            entry.modified_ns,
            entry.sha256
        ));
    }
    fs::write(path, text)
}

fn load(path: &Path) -> io::Result<BTreeMap<String, Entry>> {
    let text = fs::read_to_string(path)?;
    if !text.starts_with("# dispersal-wolves/file-watch/v1\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported baseline",
        ));
    }
    let mut output = BTreeMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid baseline row",
            ));
        }
        output.insert(
            decode_path(fields[0])?,
            Entry {
                size: fields[1]
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid size"))?,
                modified_ns: fields[2]
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid timestamp"))?,
                sha256: fields[3].to_owned(),
            },
        );
    }
    Ok(output)
}

fn compare(before: &BTreeMap<String, Entry>, after: &BTreeMap<String, Entry>) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, entry) in after {
        match before.get(path) {
            None => changes.push(Change {
                path: path.clone(),
                kind: ChangeKind::Added,
            }),
            Some(previous) if previous.sha256 != entry.sha256 || previous.size != entry.size => {
                changes.push(Change {
                    path: path.clone(),
                    kind: ChangeKind::Modified,
                })
            }
            _ => {}
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            changes.push(Change {
                path: path.clone(),
                kind: ChangeKind::Deleted,
            });
        }
    }
    changes
}

fn kind_name(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "added",
        ChangeKind::Modified => "modified",
        ChangeKind::Deleted => "deleted",
    }
}
fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
fn render(changes: &[Change], json: bool) -> String {
    if json {
        let rows = changes
            .iter()
            .map(|change| {
                format!(
                    "{{\"path\":\"{}\",\"kind\":\"{}\"}}",
                    json_escape(&change.path),
                    kind_name(&change.kind)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"schema\":\"dispersal-wolves/file-watch/v1\",\"changes\":[{rows}]}}")
    } else if changes.is_empty() {
        "No integrity changes.".into()
    } else {
        changes
            .iter()
            .map(|change| {
                format!(
                    "[{:<8}] {}",
                    kind_name(&change.kind).to_ascii_uppercase(),
                    change.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|item| item == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
fn options(args: &[String], flag: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, item)| *item == flag)
        .filter_map(|(index, _)| args.get(index + 1).cloned())
        .collect()
}
fn usage() {
    eprintln!(
        "Usage:\n  file-watch baseline PATH --output FILE [--exclude PATTERN]\n  file-watch check PATH --baseline FILE [--format text|json]\n  file-watch watch PATH --baseline FILE [--interval SECONDS]"
    );
}

fn run(args: &[String]) -> Result<u8, String> {
    if args.len() < 2 {
        usage();
        return Ok(2);
    }
    let command = &args[0];
    let root = PathBuf::from(&args[1]);
    let excludes = options(args, "--exclude");
    match command.as_str() {
        "baseline" => {
            let output = option(args, "--output").ok_or("baseline requires --output FILE")?;
            let entries = snapshot(&root, &excludes).map_err(|e| e.to_string())?;
            save(Path::new(&output), &entries).map_err(|e| e.to_string())?;
            println!("Saved {} file records to {}", entries.len(), output);
            Ok(0)
        }
        "check" => {
            let baseline = option(args, "--baseline").ok_or("check requires --baseline FILE")?;
            let before = load(Path::new(&baseline)).map_err(|e| e.to_string())?;
            let after = snapshot(&root, &excludes).map_err(|e| e.to_string())?;
            let changes = compare(&before, &after);
            println!(
                "{}",
                render(
                    &changes,
                    option(args, "--format").as_deref() == Some("json")
                )
            );
            Ok(u8::from(!changes.is_empty()))
        }
        "watch" => {
            let baseline = option(args, "--baseline").ok_or("watch requires --baseline FILE")?;
            let before = load(Path::new(&baseline)).map_err(|e| e.to_string())?;
            let interval: u64 = option(args, "--interval")
                .unwrap_or_else(|| "2".into())
                .parse()
                .map_err(|_| "interval must be a positive integer")?;
            if interval == 0 {
                return Err("interval must be positive".into());
            }
            let mut previous = Vec::new();
            loop {
                let after = snapshot(&root, &excludes).map_err(|e| e.to_string())?;
                let changes = compare(&before, &after);
                if changes != previous {
                    println!("{}", render(&changes, false));
                    previous = changes;
                }
                thread::sleep(Duration::from_secs(interval));
            }
        }
        _ => {
            usage();
            Ok(2)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("File Watch: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compares_snapshots() {
        let before = BTreeMap::from([
            (
                "a".into(),
                Entry {
                    size: 1,
                    modified_ns: 1,
                    sha256: "a".into(),
                },
            ),
            (
                "gone".into(),
                Entry {
                    size: 1,
                    modified_ns: 1,
                    sha256: "a".into(),
                },
            ),
        ]);
        let after = BTreeMap::from([
            (
                "a".into(),
                Entry {
                    size: 2,
                    modified_ns: 2,
                    sha256: "b".into(),
                },
            ),
            (
                "new".into(),
                Entry {
                    size: 1,
                    modified_ns: 1,
                    sha256: "a".into(),
                },
            ),
        ]);
        let changes = compare(&before, &after);
        assert!(
            changes
                .iter()
                .any(|c| c.path == "a" && c.kind == ChangeKind::Modified)
        );
        assert!(
            changes
                .iter()
                .any(|c| c.path == "new" && c.kind == ChangeKind::Added)
        );
        assert!(
            changes
                .iter()
                .any(|c| c.path == "gone" && c.kind == ChangeKind::Deleted)
        );
    }
    #[test]
    fn path_encoding_round_trip() {
        let value = "a%\tb\n.txt";
        assert_eq!(decode_path(&encode_path(value)).unwrap(), value);
    }
}
