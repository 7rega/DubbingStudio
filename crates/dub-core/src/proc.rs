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
