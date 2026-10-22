//! Boards: Excalidraw scenes the studio's Board view saves, one data/boards/<name>.excalidraw file each (the file
//! Excalidraw itself opens and saves). Written atomically and owner-only, a size limit per board, names checked so a
//! board can only ever be a plain file in that folder. Contract: docs/API.md, "Boards".

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::api::fail;
use crate::fsutil;
use crate::state::AppState;

/// Largest board file. Pictures dropped on a board are embedded in it, so the studio shrinks them first.
pub const MAX_BOARD_BYTES: usize = 25 << 20;
pub const MAX_BOARDS: usize = 500;
pub const MAX_NAME_CHARS: usize = 60;
const EXT: &str = ".excalidraw";

/// A board name is also its file name: 1 to 60 of A-Z, a-z, 0-9, space, -, _, ( and ), starting with a letter or
/// digit and not ending in a space. No dots or slashes, so it can never leave data/boards or hide a file.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME_CHARS
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && !n.ends_with(' ')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')'))
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct BoardInfo {
    pub name: String,
    pub bytes: u64,
    /// Unix seconds of the last save.
    pub modified: u64,
    /// The first 16 hex of the file's SHA-256: a save that names the rev it loaded is refused once the file changed.
    pub rev: String,
}

#[derive(Debug, PartialEq)]
pub enum BoardError {
    NotFound,
    /// Another board has that name (letter case aside: macOS folders ignore it).
    Exists,
    /// The board changed since the caller loaded it; carries the saved rev.
    Changed(String),
    Full,
    TooLarge,
    /// The file is not an Excalidraw scene.
    Invalid,
    Io,
}

#[derive(Clone)]
pub struct Boards {
    dir: Arc<PathBuf>,
    /// One change at a time, so a check and the write after it cannot be split by another request.
    lock: Arc<Mutex<()>>,
}

fn rev_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// An Excalidraw scene: {"type":"excalidraw","elements":[...]} with optional appState and files objects.
pub fn valid_board(v: &Value) -> bool {
    let Some(o) = v.as_object() else { return false };
    o.get("type").and_then(Value::as_str) == Some("excalidraw")
        && o.get("elements").is_some_and(Value::is_array)
        && o.get("appState").is_none_or(|a| a.is_object() || a.is_null())
        && o.get("files").is_none_or(|f| f.is_object() || f.is_null())
}

impl Boards {
    pub fn new(dir: PathBuf) -> Self {
        Boards { dir: Arc::new(dir), lock: Arc::new(Mutex::new(())) }
    }

    fn hold(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}{EXT}"))
    }

    /// The file at `name` when it is a regular file of its own (never a symlink or a hard link to another file).
    /// Only for a name as the folder lists it: on a folder that ignores letter case (macOS) the path also finds
    /// "SHOTS" for "Shots".
    fn file_at(&self, name: &str) -> Option<(PathBuf, fs::Metadata)> {
        let p = self.path(name);
        let meta = fs::symlink_metadata(&p).ok()?;
        (meta.is_file() && meta.nlink() == 1).then_some((p, meta))
    }

    /// The board saved under exactly `name`: the folder must list that name, letter case and all.
    fn file(&self, name: &str) -> Option<(PathBuf, fs::Metadata)> {
        if !self.names().iter().any(|n| n == name) {
            return None;
        }
        self.file_at(name)
    }

    /// Every board name exactly as the folder lists it.
    fn names(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&*self.dir) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(EXT)).map(str::to_string))
            .filter(|n| valid_name(n) && self.file_at(n).is_some())
            .collect()
    }

    /// The saved board whose name matches `name` but for letter case (the name itself counts too).
    fn same_name(&self, name: &str) -> Option<String> {
        self.names().into_iter().find(|n| n.eq_ignore_ascii_case(name))
    }

    fn info(&self, name: &str) -> Result<BoardInfo, BoardError> {
        self.file(name).ok_or(BoardError::NotFound)?;
        self.info_listed(name)
    }

    /// `info` for a name taken from `names()`, which already matched it against the folder.
    fn info_listed(&self, name: &str) -> Result<BoardInfo, BoardError> {
        let (meta, bytes) = self.read(name)?;
        Ok(Self::info_of(name, &meta, &bytes))
    }

    /// Reads a board's file through one handle: opened without following a symlink (O_NOFOLLOW; O_NONBLOCK so a FIFO
    /// cannot hang it), then checked on that same handle, so nothing can be swapped in between the check and the
    /// read. A file over the limit is never read in full (listing would hold the lock against saves meanwhile).
    fn read(&self, name: &str) -> Result<(fs::Metadata, Vec<u8>), BoardError> {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path(name))
            .map_err(|e| if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ELOOP) { BoardError::NotFound } else { BoardError::Io })?;
        let meta = file.metadata().map_err(|_| BoardError::Io)?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(BoardError::NotFound);
        }
        if meta.len() > MAX_BOARD_BYTES as u64 {
            return Err(BoardError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_BOARD_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| BoardError::Io)?;
        if bytes.len() > MAX_BOARD_BYTES {
            return Err(BoardError::TooLarge);
        }
        Ok((meta, bytes))
    }

    fn info_of(name: &str, meta: &fs::Metadata, bytes: &[u8]) -> BoardInfo {
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        BoardInfo { name: name.to_string(), bytes: meta.len(), modified, rev: rev_of(bytes) }
    }

    /// Every board, most recently saved first.
    pub fn list(&self) -> Vec<BoardInfo> {
        let _held = self.hold();
        let mut out: Vec<BoardInfo> = self.names().iter().filter_map(|n| self.info_listed(n).ok()).collect();
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
        out
    }

    pub fn load(&self, name: &str) -> Result<(BoardInfo, Value), BoardError> {
        let _held = self.hold();
        self.file(name).ok_or(BoardError::NotFound)?;
        let (meta, bytes) = self.read(name)?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| BoardError::Invalid)?;
        if !valid_board(&v) {
            return Err(BoardError::Invalid);
        }
        Ok((Self::info_of(name, &meta, &bytes), v))
    }

    /// Saves a board. `create`: only when no board has that name yet. `rev`: only when the saved board is still the
    /// one the caller loaded (NotFound when no board has the name any more). A name that differs from a saved board's
    /// only in letter case is refused (Exists).
    pub fn save(&self, name: &str, board: &Value, create: bool, rev: Option<&str>) -> Result<BoardInfo, BoardError> {
        if !valid_board(board) {
            return Err(BoardError::Invalid);
        }
        // Compact, as the page measures it: indenting nested arrays (freedraw points) can more than double a board.
        let bytes = serde_json::to_vec(board).map_err(|_| BoardError::Invalid)?;
        if bytes.len() > MAX_BOARD_BYTES {
            return Err(BoardError::TooLarge);
        }
        let _held = self.hold();
        match self.same_name(name) {
            Some(n) if n != name || create => return Err(BoardError::Exists),
            Some(n) => {
                if let Some(want) = rev {
                    let now = self.info(&n)?.rev;
                    if now != want {
                        return Err(BoardError::Changed(now));
                    }
                }
            }
            // The page loaded a board under this name, which was renamed or deleted since: ask, don't recreate it.
            None if rev.is_some() => return Err(BoardError::NotFound),
            None if self.names().len() >= MAX_BOARDS => return Err(BoardError::Full),
            None => {}
        }
        // A planted symlink or hard link at the name is never written through: write_private renames a new file
        // over the name, which replaces the link itself.
        fsutil::write_private(&self.path(name), &bytes).map_err(|_| BoardError::Io)?;
        self.info(name)
    }

    /// Renames a board. A new name that only changes the letter case of the old one is allowed.
    pub fn rename(&self, from: &str, to: &str) -> Result<BoardInfo, BoardError> {
        let _held = self.hold();
        self.file(from).ok_or(BoardError::NotFound)?;
        if let Some(n) = self.same_name(to) {
            if n != from {
                return Err(BoardError::Exists);
            }
        }
        fs::rename(self.path(from), self.path(to)).map_err(|_| BoardError::Io)?;
        self.info(to)
    }

    pub fn delete(&self, name: &str) -> Result<(), BoardError> {
        let _held = self.hold();
        let (p, _) = self.file(name).ok_or(BoardError::NotFound)?;
        fs::remove_file(p).map_err(|e| if e.kind() == io::ErrorKind::NotFound { BoardError::NotFound } else { BoardError::Io })
    }
}

fn board_fail(e: BoardError) -> Response {
    match e {
        BoardError::NotFound => fail(StatusCode::NOT_FOUND, "not_found"),
        BoardError::Exists => fail(StatusCode::CONFLICT, "board_exists"),
        BoardError::Changed(rev) => (StatusCode::CONFLICT, Json(json!({"error": "board_changed", "rev": rev}))).into_response(),
        BoardError::Full => fail(StatusCode::CONFLICT, "too_many_boards"),
        BoardError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "board_too_large", "max_mb": MAX_BOARD_BYTES >> 20}))).into_response(),
        BoardError::Invalid => fail(StatusCode::BAD_REQUEST, "invalid_board"),
        BoardError::Io => fail(StatusCode::INTERNAL_SERVER_ERROR, "write_failed"),
    }
}

/// A body over the route's limit is "too large"; anything else that is not the expected JSON is a bad request.
fn body_fail(e: JsonRejection) -> Response {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return board_fail(BoardError::TooLarge);
    }
    fail(StatusCode::BAD_REQUEST, "bad_request")
}

pub async fn list(State(st): State<AppState>) -> Response {
    let boards = tokio::task::spawn_blocking(move || st.boards.list()).await.unwrap_or_default();
    Json(json!({"boards": boards})).into_response()
}

#[derive(Deserialize)]
pub struct NameQuery {
    name: String,
}

pub async fn load(State(st): State<AppState>, q: Result<Query<NameQuery>, QueryRejection>) -> Response {
    let Ok(Query(q)) = q else {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    };
    if !valid_name(&q.name) {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    }
    match tokio::task::spawn_blocking(move || st.boards.load(&q.name)).await {
        Ok(Ok((info, board))) => Json(json!({"name": info.name, "rev": info.rev, "bytes": info.bytes, "modified": info.modified, "board": board})).into_response(),
        Ok(Err(BoardError::Io)) | Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "read_failed"),
        Ok(Err(e)) => board_fail(e),
    }
}

#[derive(Deserialize)]
pub struct SaveReq {
    name: String,
    board: Value,
    /// Only when no board has this name yet (New board, and saving under a new name).
    #[serde(default)]
    create: bool,
    /// The rev the page loaded; omitted, the save replaces whatever is saved.
    #[serde(default)]
    rev: Option<String>,
}

pub async fn save(State(st): State<AppState>, body: Result<Json<SaveReq>, JsonRejection>) -> Response {
    let Json(b) = match body {
        Ok(b) => b,
        Err(e) => return body_fail(e),
    };
    if !valid_name(&b.name) {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    }
    match tokio::task::spawn_blocking(move || st.boards.save(&b.name, &b.board, b.create, b.rev.as_deref())).await {
        Ok(Ok(info)) => Json(json!({"board": info})).into_response(),
        Ok(Err(e)) => board_fail(e),
        Err(_) => board_fail(BoardError::Io),
    }
}

#[derive(Deserialize)]
pub struct RenameReq {
    from: String,
    to: String,
}

pub async fn rename(State(st): State<AppState>, body: Result<Json<RenameReq>, JsonRejection>) -> Response {
    let Json(b) = match body {
        Ok(b) => b,
        Err(e) => return body_fail(e),
    };
    if !valid_name(&b.from) || !valid_name(&b.to) {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    }
    match tokio::task::spawn_blocking(move || st.boards.rename(&b.from, &b.to)).await {
        Ok(Ok(info)) => Json(json!({"board": info})).into_response(),
        Ok(Err(e)) => board_fail(e),
        Err(_) => board_fail(BoardError::Io),
    }
}

#[derive(Deserialize)]
pub struct DeleteReq {
    name: String,
}

pub async fn delete(State(st): State<AppState>, body: Result<Json<DeleteReq>, JsonRejection>) -> Response {
    let Json(b) = match body {
        Ok(b) => b,
        Err(e) => return body_fail(e),
    };
    if !valid_name(&b.name) {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    }
    match tokio::task::spawn_blocking(move || st.boards.delete(&b.name)).await {
        Ok(Ok(())) => Json(json!({"ok": true})).into_response(),
        Ok(Err(e)) => board_fail(e),
        Err(_) => board_fail(BoardError::Io),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    fn scene(text: &str) -> Value {
        json!({"type": "excalidraw", "version": 2, "source": "agave", "elements": [{"id": "t1", "type": "text", "text": text}],
               "appState": {"viewBackgroundColor": "#ffffff"}, "files": {}})
    }

    #[test]
    fn names_are_plain_file_names() {
        for ok in ["Shots", "shot list 2", "Ep-1_take(3)", "9", &"a".repeat(60)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", " lead", "trail ", ".hidden", "a.b", "../up", "a/b", "a\\b", "café", "tab\there", "nul\0", "-x", &"a".repeat(61)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn boards_save_load_rename_and_delete() {
        let dir = fsutil::test_dir("boards_store").join("boards");
        let b = Boards::new(dir.clone());
        assert_eq!(b.list(), vec![], "no folder yet: no boards");
        let first = b.save("Shots", &scene("a cat"), true, None).unwrap();
        assert_eq!(first.rev.len(), 16);
        assert_eq!(fs::metadata(dir.join("Shots.excalidraw")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(b.save("Shots", &scene("b"), true, None), Err(BoardError::Exists), "create never replaces");
        assert_eq!(b.save("shots", &scene("b"), false, None), Err(BoardError::Exists), "letter case alone is the same name");
        let (info, v) = b.load("Shots").unwrap();
        assert_eq!((info.rev.as_str(), v["elements"][0]["text"].as_str()), (first.rev.as_str(), Some("a cat")));
        // A save naming the rev it loaded goes through; one naming an older rev is refused with the saved rev.
        let second = b.save("Shots", &scene("a dog"), false, Some(&first.rev)).unwrap();
        assert_ne!(second.rev, first.rev);
        assert_eq!(b.save("Shots", &scene("x"), false, Some(&first.rev)), Err(BoardError::Changed(second.rev.clone())));
        assert!(b.save("Shots", &scene("x"), false, None).is_ok(), "no rev: the save replaces it");
        // A rev for a board renamed or deleted elsewhere is refused, so the page can ask instead of recreating it.
        assert_eq!(b.save("Gone", &scene("x"), false, Some(&first.rev)), Err(BoardError::NotFound));
        assert_eq!(b.load("Gone").err(), Some(BoardError::NotFound), "nothing was written");
        // Rename: not over another board, but a change of letter case is fine.
        b.save("Other", &scene("o"), true, None).unwrap();
        assert_eq!(b.rename("Shots", "other"), Err(BoardError::Exists));
        assert_eq!(b.rename("Nope", "Fresh"), Err(BoardError::NotFound));
        assert_eq!(b.rename("Shots", "SHOTS").unwrap().name, "SHOTS");
        // The old spelling is gone even on a folder that ignores letter case (macOS): names are matched against
        // the folder's own listing, never found through the path.
        assert_eq!(b.load("Shots").err(), Some(BoardError::NotFound));
        assert_eq!(b.delete("Shots"), Err(BoardError::NotFound));
        assert_eq!(b.rename("Shots", "Fresh"), Err(BoardError::NotFound));
        assert!(b.load("SHOTS").is_ok());
        let names: Vec<String> = b.list().into_iter().map(|i| i.name).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"SHOTS".to_string()) && names.contains(&"Other".to_string()));
        assert_eq!(b.delete("SHOTS"), Ok(()));
        assert_eq!(b.delete("SHOTS"), Err(BoardError::NotFound));
        assert_eq!(b.save("Bad", &json!({"type": "something", "elements": []}), true, None), Err(BoardError::Invalid));
        assert_eq!(b.save("Bad", &json!({"type": "excalidraw", "elements": {}}), true, None), Err(BoardError::Invalid));
        // No temp files are left beside the boards.
        let left: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["Other.excalidraw".to_string()]);
    }

    #[test]
    fn links_and_stray_files_are_not_boards() {
        let dir = fsutil::test_dir("boards_links").join("boards");
        let b = Boards::new(dir.clone());
        b.save("Real", &scene("r"), true, None).unwrap();
        let outside = dir.parent().unwrap().join("secret.json");
        fs::write(&outside, serde_json::to_vec(&scene("secret")).unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("Linked.excalidraw")).unwrap();
        fs::hard_link(&outside, dir.join("Hard.excalidraw")).unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        fs::write(dir.join("bad.name.excalidraw"), b"{}").unwrap();
        fs::write(dir.join("Broken.excalidraw"), b"{not json").unwrap();
        let names: Vec<String> = b.list().into_iter().map(|i| i.name).collect();
        assert!(names.contains(&"Real".to_string()) && names.contains(&"Broken".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n == "Linked" || n == "Hard"), "{names:?}");
        assert_eq!(b.load("Linked").err(), Some(BoardError::NotFound));
        assert_eq!(b.load("Hard").err(), Some(BoardError::NotFound));
        assert_eq!(b.load("Broken").err(), Some(BoardError::Invalid));
        assert_eq!(b.delete("Linked"), Err(BoardError::NotFound));
        // Saving over a planted link replaces the link, never the file it points to.
        b.save("Linked", &scene("mine"), false, None).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(&outside).unwrap()).unwrap()["elements"][0]["text"], "secret");
        assert!(!fs::symlink_metadata(dir.join("Linked.excalidraw")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_board_is_read_through_one_handle_that_never_follows_a_link() {
        let dir = fsutil::test_dir("boards_handle").join("boards");
        let b = Boards::new(dir.clone());
        b.save("Real", &scene("r"), true, None).unwrap();
        assert!(b.read("Real").is_ok());
        // Even when the name was checked first, the read itself refuses a link put there since.
        let outside = dir.parent().unwrap().join("secret.json");
        fs::write(&outside, serde_json::to_vec(&scene("secret")).unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("Swapped.excalidraw")).unwrap();
        assert_eq!(b.read("Swapped").err(), Some(BoardError::NotFound));
        // A FIFO fails at once instead of blocking (on a thread, so a regression fails instead of hanging).
        let fifo = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.join("Pipe.excalidraw").as_os_str())).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(b.read("Pipe").is_err()));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true));
    }

    #[test]
    fn files_over_the_limit_are_not_listed_or_read() {
        let dir = fsutil::test_dir("boards_oversize").join("boards");
        let b = Boards::new(dir.clone());
        b.save("Small", &scene("s"), true, None).unwrap();
        // A sparse file: over the limit on paper, nothing to read on disk.
        let f = fs::File::create(dir.join("Huge.excalidraw")).unwrap();
        f.set_len(300 << 20).unwrap();
        let names: Vec<String> = b.list().into_iter().map(|i| i.name).collect();
        assert_eq!(names, vec!["Small".to_string()]);
        assert_eq!(b.load("Huge").err(), Some(BoardError::TooLarge));
        assert_eq!(b.save("huge", &scene("x"), false, None), Err(BoardError::Exists), "its name is still taken");
    }

    #[test]
    fn boards_have_a_size_limit() {
        let b = Boards::new(fsutil::test_dir("boards_size").join("boards"));
        let mut big = scene("x");
        big["files"] = json!({"f1": {"id": "f1", "mimeType": "image/png", "dataURL": "x".repeat(MAX_BOARD_BYTES)}});
        assert_eq!(b.save("Big", &big, true, None), Err(BoardError::TooLarge));
        assert_eq!(b.list(), vec![]);
    }

    #[tokio::test]
    async fn a_board_just_under_the_limit_saves_however_many_small_elements_it_has() {
        let app = crate::api::router(AppState::for_tests("boards_near_limit"));
        // Freedraw strokes: many short nested arrays, which pretty printing would blow up well past 25 MB.
        let stroke = |i: usize| json!({"id": format!("e{i}"), "type": "freedraw", "points": vec![[0, 1]; 40]});
        let mut elements = Vec::new();
        let mut size = json!({"name": "Near", "board": {"type": "excalidraw", "elements": []}}).to_string().len();
        loop {
            let e = stroke(elements.len());
            let n = serde_json::to_vec(&e).unwrap().len() + 1;
            if size + n >= MAX_BOARD_BYTES - 4096 {
                break;
            }
            size += n;
            elements.push(e);
        }
        let body = json!({"name": "Near", "board": {"type": "excalidraw", "elements": elements}}).to_string();
        assert!(body.len() > MAX_BOARD_BYTES - 8192 && body.len() < MAX_BOARD_BYTES, "{}", body.len());
        assert!(serde_json::to_vec_pretty(&serde_json::from_str::<Value>(&body).unwrap()).unwrap().len() > MAX_BOARD_BYTES);
        let (s, v) = send(&app, post("/api/boards/save", body)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["board"]["bytes"].as_u64().unwrap() < MAX_BOARD_BYTES as u64);
    }

    fn post(uri: &str, body: String) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:4170")
            .header("content-type", "application/json")
            .header("cookie", format!("agave_launch_4170={}", crate::state::TEST_LAUNCH_TOKEN))
            .body(Body::from(body))
            .unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).header("host", "127.0.0.1:4170").body(Body::empty()).unwrap()
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
        let resp = app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn board_routes_list_load_save_rename_and_delete() {
        let st = AppState::for_tests("boards_routes");
        let app = crate::api::router(st.clone());
        let (s, v) = send(&app, get("/api/boards")).await;
        assert_eq!((s, v), (StatusCode::OK, json!({"boards": []})));
        let body = json!({"name": "Ep 1", "board": scene("a fox runs"), "create": true}).to_string();
        let (s, v) = send(&app, post("/api/boards/save", body.clone())).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let rev = v["board"]["rev"].as_str().unwrap().to_string();
        let (s, v) = send(&app, post("/api/boards/save", body)).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::CONFLICT, Some("board_exists")));
        let (s, v) = send(&app, get("/api/boards/load?name=Ep%201")).await;
        assert_eq!((s, v["rev"].as_str(), v["board"]["elements"][0]["text"].as_str()), (StatusCode::OK, Some(rev.as_str()), Some("a fox runs")));
        let (_, v) = send(&app, get("/api/boards")).await;
        assert_eq!(v["boards"][0]["name"], "Ep 1");
        // A stale rev is refused and the answer carries the saved one.
        let save = |rev: &str| json!({"name": "Ep 1", "board": scene("v2"), "rev": rev}).to_string();
        let (s, v) = send(&app, post("/api/boards/save", save(&rev))).await;
        assert_eq!(s, StatusCode::OK);
        let rev2 = v["board"]["rev"].as_str().unwrap().to_string();
        let (s, v) = send(&app, post("/api/boards/save", save(&rev))).await;
        assert_eq!((s, v["error"].as_str(), v["rev"].as_str()), (StatusCode::CONFLICT, Some("board_changed"), Some(rev2.as_str())));
        // Names are checked on every route; nothing outside data/boards can be named.
        for bad in ["../keys", ".x", "a/b", ""] {
            let q = format!("/api/boards/load?name={}", bad.replace('/', "%2F"));
            let (s, v) = send(&app, get(&q)).await;
            assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_name")), "{bad}");
            let (s, _) = send(&app, post("/api/boards/save", json!({"name": bad, "board": scene("x")}).to_string())).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
            let (s, _) = send(&app, post("/api/boards/delete", json!({"name": bad}).to_string())).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
        }
        let (s, v) = send(&app, get("/api/boards/load")).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_name")));
        let (s, v) = send(&app, post("/api/boards/save", json!({"name": "X", "board": {"type": "x"}}).to_string())).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_board")));
        let (s, v) = send(&app, get("/api/boards/load?name=Missing")).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
        let (s, v) = send(&app, post("/api/boards/rename", json!({"from": "Ep 1", "to": "Episode 1"}).to_string())).await;
        assert_eq!((s, v["board"]["name"].as_str()), (StatusCode::OK, Some("Episode 1")));
        let (s, v) = send(&app, post("/api/boards/delete", json!({"name": "Episode 1"}).to_string())).await;
        assert_eq!((s, v), (StatusCode::OK, json!({"ok": true})));
        let (s, _) = send(&app, post("/api/boards/delete", json!({"name": "Episode 1"}).to_string())).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // A body over the limit is refused as too large before it is read in full.
        let huge = format!("{{\"name\":\"Huge\",\"board\":{{\"type\":\"excalidraw\",\"elements\":[],\"pad\":\"{}\"}}}}", "x".repeat(MAX_BOARD_BYTES + 1024));
        let (s, v) = send(&app, post("/api/boards/save", huge)).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::PAYLOAD_TOO_LARGE, Some("board_too_large")));
    }

    #[tokio::test]
    async fn board_changes_need_the_page_and_reads_refuse_other_sites() {
        let app = crate::api::router(AppState::for_tests("boards_guard"));
        let body = json!({"name": "Ep", "board": scene("x")}).to_string();
        // No launch cookie: a script that only calls the API cannot write a board.
        let mut req = post("/api/boards/save", body.clone());
        req.headers_mut().remove("cookie");
        let (s, v) = send(&app, req).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::FORBIDDEN, Some("launch_token")));
        // Not JSON: refused before the handler.
        let mut req = post("/api/boards/delete", json!({"name": "Ep"}).to_string());
        req.headers_mut().insert("content-type", "text/plain".parse().unwrap());
        let (s, _) = send(&app, req).await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        // Another site cannot read a board, and another host name is refused.
        let mut req = get("/api/boards");
        req.headers_mut().insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
        let mut req = get("/api/boards/load?name=Ep");
        req.headers_mut().insert("host", "evil.test:4170".parse().unwrap());
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
        let mut req = post("/api/boards/save", body);
        req.headers_mut().insert("origin", "http://evil.test".parse().unwrap());
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    }
}
