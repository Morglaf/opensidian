// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas-core: the file I/O of the Canvas core plugin (JSON Canvas 1.0,
   https://jsoncanvas.org/spec/1.0/). Canvas is an APP feature, not a
   community plugin, so it works in restricted mode — and therefore gets its
   own narrow surface instead of plugin_vfs (which an untrusted vault answers
   with EPERM):

     list            every regular `*.canvas` file in the vault, vault-relative,
                     sorted; hidden components (.obsidian, .trash, ...) and
                     symlinks are never walked or listed
     read  <path>    the file's text (UTF-8, at most CANVAS_MAX bytes)
     write <path>    atomic replace of an EXISTING canvas file
     create <path>   a NEW canvas file (EEXIST if anything is there); the
                     parent folder must exist

   Every path goes through vaultfs::plugin_rel (no absolute / dot / NUL /
   backslash / percent-encoded forms) and must name a `.canvas` leaf with no
   hidden component, so this command can never touch a note, a config file
   or anything outside the vault. The bytes are never reformatted here:
   whatever text the view hands over is what lands on disk, and a read
   returns the file exactly as stored (byte-stable round trip). */

use crate::vaultfs::{self, Vault};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

/// the largest canvas file read or written (a canvas is JSON text)
pub const CANVAS_MAX: u64 = 16 << 20;
pub const EXT: &str = ".canvas";

fn err(op: &str, path: &str, why: &str) -> String {
    format!("{why}, canvas {op} '{path}'")
}

/// vault-relative path of a canvas file, or the refusal
pub fn canvas_rel(op: &str, path: &str) -> Result<PathBuf, String> {
    let rel = vaultfs::plugin_rel(path).map_err(|e| vaultfs::plugin_err(op, path, &e))?;
    let comps: Vec<&std::ffi::OsStr> = rel.iter().collect();
    let Some(leaf) = comps.last().and_then(|c| c.to_str()) else {
        return Err(err(op, path, "EINVAL: not a canvas path"));
    };
    if comps.iter().any(|c| c.as_encoded_bytes().first() == Some(&b'.')) {
        return Err(err(op, path, "EPERM: hidden path component"));
    }
    if !leaf.ends_with(EXT) || leaf.len() == EXT.len() {
        return Err(err(op, path, "EINVAL: not a .canvas file"));
    }
    Ok(rel)
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>, depth: usize) {
    if depth >= vaultfs::MAX_WALK_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with('.') {
            continue;
        }
        // DirEntry::file_type does not follow symlinks: a link is neither
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_dir() {
            walk(&p, base, out, depth + 1);
        } else if t.is_file() && n.ends_with(EXT) && n.len() > EXT.len() {
            if let Ok(rel) = p.strip_prefix(base) {
                if let Some(s) = rel.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
}

pub fn list(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(c) = root.canonicalize() {
        walk(&c, &c, &mut out, 0);
    }
    out.sort();
    out
}

fn exists(v: &Vault, rel: &Path) -> io::Result<bool> {
    match v.p_stat(rel) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn op(root: &Path, op: &str, path: &str, data: Option<&str>) -> Result<Value, String> {
    if op == "list" {
        return Ok(json!(list(root)));
    }
    let rel = canvas_rel(op, path)?;
    let e = |x: io::Error| vaultfs::plugin_err(op, path, &x);
    let v = Vault::open(root).map_err(e)?;
    let body = || -> Result<&[u8], String> {
        let d = data.ok_or_else(|| err(op, path, "EINVAL: missing data"))?;
        if d.len() as u64 > CANVAS_MAX {
            return Err(err(op, path, "EFBIG: canvas too large"));
        }
        Ok(d.as_bytes())
    };
    match op {
        "read" => {
            let b = v.p_read(&rel, CANVAS_MAX).map_err(e)?;
            String::from_utf8(b).map(Value::String).map_err(|_| err(op, path, "EILSEQ: not UTF-8 text"))
        }
        "write" => {
            let b = body()?;
            match v.p_stat(&rel).map_err(e)? {
                s if s.kind == "file" => {}
                _ => return Err(err(op, path, "EISDIR: not a file")),
            }
            v.p_write(&rel, b, false).map(|_| Value::Null).map_err(e)
        }
        "create" => {
            let b = body()?;
            if exists(&v, &rel).map_err(e)? {
                return Err(err(op, path, "EEXIST: file already exists"));
            }
            v.p_write(&rel, b, false).map(|_| Value::Null).map_err(e)
        }
        _ => Err(format!("ENOSYS: canvas op {op:?} does not exist")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn fresh(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("canvasfs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, out) = (base.join("vault"), base.join("outside"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&out).unwrap();
        (root, out)
    }

    #[test]
    fn canvasfs_read_is_byte_exact_and_write_replaces() {
        let (root, _) = fresh("rw");
        // odd spacing, key order, trailing newline: read hands it back verbatim
        let raw = "{\"nodes\":[ {\"id\":\"a\",\"type\":\"text\",\"text\":\"h\u{e9}\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"zz\":7}],\n \"edges\":[]}\n";
        fs::write(root.join("A.canvas"), raw).unwrap();
        assert_eq!(op(&root, "read", "A.canvas", None).unwrap(), Value::String(raw.into()));
        op(&root, "write", "A.canvas", Some("{}")).unwrap();
        assert_eq!(fs::read_to_string(root.join("A.canvas")).unwrap(), "{}");
    }

    #[test]
    fn canvasfs_create_new_only_and_write_existing_only() {
        let (root, _) = fresh("create");
        fs::create_dir_all(root.join("sub")).unwrap();
        op(&root, "create", "sub/N.canvas", Some("{\"nodes\":[],\"edges\":[]}")).unwrap();
        assert_eq!(fs::read_to_string(root.join("sub/N.canvas")).unwrap(), "{\"nodes\":[],\"edges\":[]}");
        let e = op(&root, "create", "sub/N.canvas", Some("{}")).unwrap_err();
        assert!(e.starts_with("EEXIST"), "{e}");
        assert_eq!(fs::read_to_string(root.join("sub/N.canvas")).unwrap(), "{\"nodes\":[],\"edges\":[]}");
        assert!(op(&root, "write", "Ghost.canvas", Some("{}")).is_err());
        assert!(!root.join("Ghost.canvas").exists());
        // the parent is not created
        assert!(op(&root, "create", "nodir/X.canvas", Some("{}")).is_err());
        assert!(!root.join("nodir").exists());
    }

    #[test]
    fn canvasfs_refuses_non_canvas_hidden_and_escaping_paths() {
        let (root, out) = fresh("refuse");
        fs::write(root.join("Note.md"), "keep").unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(out.join("V.canvas"), "outside").unwrap();
        for p in ["Note.md", "Note.md.bak", ".canvas", ".obsidian/x.canvas", ".hidden.canvas", "../outside/V.canvas",
                  "/etc/x.canvas", "a/../B.canvas", "a\\b.canvas", "%2e%2e/x.canvas", "x.canvas\0", ""] {
            assert!(op(&root, "read", p, None).is_err(), "read {p:?}");
            assert!(op(&root, "write", p, Some("{}")).is_err(), "write {p:?}");
            assert!(op(&root, "create", p, Some("{}")).is_err(), "create {p:?}");
        }
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "keep");
        assert!(!root.join(".obsidian/x.canvas").exists());
        assert_eq!(fs::read_to_string(out.join("V.canvas")).unwrap(), "outside");
        assert!(op(&root, "unlink", "A.canvas", None).unwrap_err().starts_with("ENOSYS"));
    }

    #[test]
    fn canvasfs_symlinks_are_never_followed() {
        let (root, out) = fresh("link");
        fs::write(out.join("T.canvas"), "outside").unwrap();
        symlink(out.join("T.canvas"), root.join("L.canvas")).unwrap();
        symlink(&out, root.join("d")).unwrap();
        assert!(op(&root, "read", "L.canvas", None).is_err());
        assert!(op(&root, "write", "L.canvas", Some("pwn")).is_err());
        assert!(op(&root, "read", "d/T.canvas", None).is_err());
        assert!(op(&root, "write", "d/T.canvas", Some("pwn")).is_err());
        assert!(op(&root, "create", "d/U.canvas", Some("pwn")).is_err());
        assert_eq!(fs::read_to_string(out.join("T.canvas")).unwrap(), "outside");
        assert!(!out.join("U.canvas").exists());
        assert_eq!(list(&root), Vec::<String>::new());
    }

    #[test]
    fn canvasfs_list_finds_canvas_files_only() {
        let (root, _) = fresh("list");
        fs::create_dir_all(root.join("b/c")).unwrap();
        fs::create_dir_all(root.join(".trash")).unwrap();
        for p in ["Z.canvas", "b/c/Y.canvas", "b/n.md", ".trash/old.canvas", "b/.h.canvas", "b/x.canvas.md"] {
            fs::write(root.join(p), "{}").unwrap();
        }
        fs::create_dir_all(root.join("dir.canvas")).unwrap();
        assert_eq!(list(&root), vec!["Z.canvas".to_string(), "b/c/Y.canvas".to_string()]);
        assert_eq!(op(&root, "list", "", None).unwrap(), json!(["Z.canvas", "b/c/Y.canvas"]));
    }

    #[test]
    fn canvasfs_rejects_non_utf8_and_oversized() {
        let (root, _) = fresh("big");
        fs::write(root.join("B.canvas"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(op(&root, "read", "B.canvas", None).unwrap_err().starts_with("EILSEQ"));
        let big = "x".repeat(CANVAS_MAX as usize + 1);
        assert!(op(&root, "write", "B.canvas", Some(&big)).unwrap_err().starts_with("EFBIG"));
        assert_eq!(fs::read(root.join("B.canvas")).unwrap(), vec![0xff, 0xfe, 0x00]);
        assert!(op(&root, "write", "B.canvas", None).is_err());
    }
}
