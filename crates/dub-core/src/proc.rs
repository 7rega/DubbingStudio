//! Учёт дочерних процессов (ffmpeg, сепаратор, llama-server, whisper). Сервер ставит хук, который
//! относит процесс к выполняемой джобе, чтобы её отмена могла этот процесс убить. Без хука учёт ничего
//! не делает.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// Хук учёта: `alive=true` — процесс запущен, `false` — завершён и дождан.
pub type ChildHook = fn(pid: u32, alive: bool);

static HOOK: OnceLock<ChildHook> = OnceLock::new();

/// Поставить хук (один раз на процесс; повторные вызовы игнорируются).
pub fn set_hook(hook: ChildHook) {
    let _ = HOOK.set(hook);
}

/// Перенос привязки к джобе в новый поток: вызывается в порождающем потоке и возвращает то, что в новом
/// потоке привязывает его к той же джобе; результат держится, пока поток работает.
pub type Carry = fn() -> Box<dyn FnOnce() -> Box<dyn std::any::Any> + Send>;

static CARRY: OnceLock<Carry> = OnceLock::new();

/// Поставить перенос привязки (один раз на процесс; повторные вызовы игнорируются).
pub fn set_carry(carry: Carry) {
    let _ = CARRY.set(carry);
}

/// `std::thread::spawn`, после которого процессы, запущенные в новом потоке, учитываются за той же
/// джобой, что и у порождающего потока.
pub fn spawn<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    let attach = CARRY.get().map(|carry| carry());
    std::thread::spawn(move || {
        let _attached = attach.map(|attach| attach());
        work()
    })
}

/// Отметка «процесс жив»; снимается при drop. Держать, пока хэндл процесса не закрыт: тогда pid не
/// может быть переиспользован другим процессом.
pub struct ChildGuard {
    pid: u32,
}

/// ffmpeg первым из каталога tools/ffmpeg / ffmpeg или из PATH процесса.
pub fn ffmpeg() -> std::path::PathBuf {
    tool("ffmpeg")
}

/// ffprobe первым из каталога tools/ffmpeg / ffmpeg или из PATH процесса.
pub fn ffprobe() -> std::path::PathBuf {
    tool("ffprobe")
}

fn tool(name: &str) -> std::path::PathBuf {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    for dir in ["tools/ffmpeg", "ffmpeg"] {
        let p = std::path::PathBuf::from(dir).join(&file);
        if p.is_file() {
            return p;
        }
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|dir| dir.join(&file)).find(|path| path.is_file()))
        .unwrap_or_else(|| std::path::PathBuf::from(file))
}

pub fn track(pid: u32) -> ChildGuard {
    if let Some(h) = HOOK.get() {
        h(pid, true);
    }
    ChildGuard { pid }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(h) = HOOK.get() {
            h(self.pid, false);
        }
    }
}

/// Аналог `Command::output()` с учётом процесса: stdin закрыт, stdout/stderr читаются целиком.
pub fn output(cmd: &mut Command) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    let guard = track(child.id());
    let out_pipe = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = out_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let mut stderr = Vec::new();
    if let Some(mut p) = child.stderr.take() {
        let _ = p.read_to_end(&mut stderr);
    }
    let status = child.wait();
    drop(guard);
    let stdout = reader.join().unwrap_or_default();
    Ok(Output { status: status?, stdout, stderr })
}
