//! Загрузчик рантайма transcribe.cpp (transcribe_engine.dll) и детектора языка VoxLingua107.
//!
//! Библиотека грузится динамически через `libloading` в рантайме.

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use libloading::Library;

pub const LANGID_MIN_SAMPLES: usize = 8_000;

#[repr(C)]
struct LoadParams {
    struct_size: u64,
    backend: c_int,
    device: *mut std::ffi::c_void,
    no_mmap: bool,
}

#[repr(C)]
struct LangIdParams {
    struct_size: u64,
    allowed: *const *const c_char,
    n_allowed: i32,
}

#[repr(C)]
struct LangIdResult {
    struct_size: u64,
    n_candidates: i32,
    allowed_mass: f32,
}

#[repr(C)]
struct LangIdCandidate {
    struct_size: u64,
    index: i32,
    code: *const c_char,
    name: *const c_char,
    p: f32,
    logit: f32,
}

const ABI_MODEL_LOAD_PARAMS: c_int = 0;
const ABI_LANGID_PARAMS: c_int = 21;
const ABI_LANGID_RESULT: c_int = 22;
const ABI_LANGID_CANDIDATE: c_int = 23;
const BACKEND_CPU: c_int = 1;

enum RawModel {}
enum RawLangId {}

#[derive(Clone, Copy)]
struct Api {
    status_string: unsafe extern "C" fn(c_int) -> *const c_char,
    abi_size: unsafe extern "C" fn(c_int) -> usize,
    load: unsafe extern "C" fn(*const c_char, *const LoadParams, *mut *mut RawModel) -> c_int,
    load_params_init: unsafe extern "C" fn(*mut LoadParams),
    model_free: unsafe extern "C" fn(*mut RawModel),
    langid_session_init: unsafe extern "C" fn(*mut RawModel, *const std::ffi::c_void, *mut *mut RawLangId) -> c_int,
    langid_session_free: unsafe extern "C" fn(*mut RawLangId),
    langid_params_init: unsafe extern "C" fn(*mut LangIdParams),
    langid_run: unsafe extern "C" fn(*mut RawLangId, *const f32, c_int, *const LangIdParams) -> c_int,
    langid_result_init: unsafe extern "C" fn(*mut LangIdResult),
    langid_get_result: unsafe extern "C" fn(*const RawLangId, *mut LangIdResult) -> c_int,
    langid_candidate_init: unsafe extern "C" fn(*mut LangIdCandidate),
    langid_get_candidate: unsafe extern "C" fn(*const RawLangId, c_int, *mut LangIdCandidate) -> c_int,
}

static LOADED: Mutex<Vec<(PathBuf, Library, Api)>> = Mutex::new(Vec::new());

fn open_library(path: &Path) -> Result<Library, String> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{Library as WindowsLibrary, LOAD_WITH_ALTERED_SEARCH_PATH};
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cd| cd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        unsafe { WindowsLibrary::load_with_flags(&full, LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map(Library::from)
            .map_err(|e| format!("{}: {e}", full.display()))
    }
    #[cfg(not(windows))]
    unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))
}

fn get_api(engine_path: &Path) -> Result<Api, String> {
    let dll_path = if engine_path.is_file() {
        engine_path.to_path_buf()
    } else {
        let name = if cfg!(windows) { "transcribe_engine.dll" } else { "libtranscribe_engine.so" };
        engine_path.join(name)
    };

    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, _, api)) = loaded.iter().find(|(p, _, _)| *p == dll_path) {
        return Ok(*api);
    }

    if !dll_path.is_file() {
        return Err(format!("transcribe.cpp: библиотеку не найдено: {}", dll_path.display()));
    }

    let lib = open_library(&dll_path)?;
    let api = unsafe {
        macro_rules! sym {
            ($name:literal) => {
                *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("transcribe.cpp {}: {e}", $name))?
            };
        }
        Api {
            status_string: sym!("transcribe_status_string"),
            abi_size: sym!("transcribe_abi_struct_size"),
            load: sym!("transcribe_model_load_file"),
            load_params_init: sym!("transcribe_model_load_params_init"),
            model_free: sym!("transcribe_model_free"),
            langid_session_init: sym!("transcribe_langid_session_init"),
            langid_session_free: sym!("transcribe_langid_session_free"),
            langid_params_init: sym!("transcribe_langid_params_init"),
            langid_run: sym!("transcribe_langid_run"),
            langid_result_init: sym!("transcribe_langid_result_init"),
            langid_get_result: sym!("transcribe_langid_get_result"),
            langid_candidate_init: sym!("transcribe_langid_candidate_init"),
            langid_get_candidate: sym!("transcribe_langid_get_candidate"),
        }
    };

    // Валидация ABI-размеров структур
    for (which, ours, name) in [
        (ABI_LANGID_PARAMS, std::mem::size_of::<LangIdParams>(), "langid params"),
        (ABI_LANGID_RESULT, std::mem::size_of::<LangIdResult>(), "langid result"),
        (ABI_LANGID_CANDIDATE, std::mem::size_of::<LangIdCandidate>(), "langid candidate"),
    ] {
        let theirs = unsafe { (api.abi_size)(which) };
        if theirs != ours {
            return Err(format!("transcribe.cpp {}: структура {name} имеет размер {theirs} байт, ожидалось {ours}", dll_path.display()));
        }
    }
    let load = unsafe { (api.abi_size)(ABI_MODEL_LOAD_PARAMS) };
    if load < std::mem::offset_of!(LoadParams, no_mmap) || load > std::mem::size_of::<LoadParams>() {
        return Err(format!("transcribe.cpp {}: структура load params имеет размер {load} байт, ожидалось {}", dll_path.display(), std::mem::size_of::<LoadParams>()));
    }

    loaded.push((dll_path, lib, api));
    Ok(api)
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

struct LangIdSession {
    api: Api,
    model: *mut RawModel,
    session: *mut RawLangId,
}

unsafe impl Send for LangIdSession {}

impl Drop for LangIdSession {
    fn drop(&mut self) {
        unsafe {
            (self.api.langid_session_free)(self.session);
            (self.api.model_free)(self.model);
        }
    }
}

static LANGID_LOADED: Mutex<Option<((PathBuf, PathBuf), LangIdSession)>> = Mutex::new(None);

fn load_langid_model(api: &Api, model_path: &Path) -> Result<*mut RawModel, String> {
    if !model_path.is_file() {
        return Err(format!("transcribe.cpp: файл модели не найден: {}", model_path.display()));
    }
    let c_path = CString::new(model_path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let mut raw: *mut RawModel = std::ptr::null_mut();
    let mut lp = std::mem::MaybeUninit::<LoadParams>::zeroed();
    let s = unsafe {
        (api.load_params_init)(lp.as_mut_ptr());
        let mut params = lp.assume_init();
        params.backend = BACKEND_CPU;
        (api.load)(c_path.as_ptr(), &params, &mut raw)
    };
    if s != 0 || raw.is_null() {
        return Err(format!("transcribe.cpp {}: {}", model_path.display(), text(unsafe { (api.status_string)(s) })));
    }
    Ok(raw)
}

/// Ответ детектора языка VoxLingua107
#[derive(Debug, Clone)]
pub struct LangGuess {
    pub ranked: Vec<(String, f32)>,
    pub allowed_mass: f32,
}

/// Определение языка речи 16 кГц моно по модели VoxLingua107.
/// `engine_path`: путь к `transcribe_engine.dll` или папке с ним.
/// `model_path`: путь к GGUF-файлу VoxLingua107.
/// `pcm`: 16 кГц моно сэмплы f32 (рекомендуется первые 30 секунд).
pub fn detect_language(engine_path: &Path, model_path: &Path, pcm: &[f32], allowed: &[&str]) -> Result<LangGuess, String> {
    if pcm.len() < LANGID_MIN_SAMPLES {
        return Err("аудио слишком короткое для надёжного определения языка (требуется от 0.5 с)".into());
    }

    let api = get_api(engine_path)?;
    let key = (engine_path.to_path_buf(), model_path.to_path_buf());
    let mut slot = LANGID_LOADED.lock().unwrap_or_else(|e| e.into_inner());

    if slot.as_ref().is_none_or(|(k, _)| *k != key) {
        *slot = None;
        let raw = load_langid_model(&api, model_path)?;
        let mut session: *mut RawLangId = std::ptr::null_mut();
        let s = unsafe { (api.langid_session_init)(raw, std::ptr::null(), &mut session) };
        if s != 0 || session.is_null() {
            unsafe { (api.model_free)(raw) };
            return Err(format!("transcribe.cpp {}: {}", model_path.display(), text(unsafe { (api.status_string)(s) })));
        }
        *slot = Some((key, LangIdSession { api, model: raw, session }));
    }

    let id = &slot.as_ref().expect("loaded above").1;
    let codes: Vec<CString> = allowed.iter().map(|c| CString::new(*c).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    let ptrs: Vec<*const c_char> = codes.iter().map(|c| c.as_ptr()).collect();

    let mut lp = std::mem::MaybeUninit::<LangIdParams>::zeroed();
    let params = unsafe {
        (api.langid_params_init)(lp.as_mut_ptr());
        let mut params = lp.assume_init();
        if !ptrs.is_empty() {
            params.allowed = ptrs.as_ptr();
            params.n_allowed = ptrs.len() as i32;
        }
        params
    };

    let s = unsafe { (api.langid_run)(id.session, pcm.as_ptr(), pcm.len() as c_int, &params) };
    if s != 0 {
        return Err(format!("transcribe.cpp langid error: {}", text(unsafe { (api.status_string)(s) })));
    }

    let mut r = std::mem::MaybeUninit::<LangIdResult>::zeroed();
    let (n, allowed_mass) = unsafe {
        (api.langid_result_init)(r.as_mut_ptr());
        if (api.langid_get_result)(id.session, r.as_mut_ptr()) != 0 {
            return Err("transcribe.cpp langid: результат не получен".into());
        }
        let r = r.assume_init();
        (r.n_candidates.max(0), r.allowed_mass)
    };

    let mut out = Vec::new();
    for i in 0..n.min(5) {
        let mut c = std::mem::MaybeUninit::<LangIdCandidate>::zeroed();
        unsafe {
            (api.langid_candidate_init)(c.as_mut_ptr());
            if (api.langid_get_candidate)(id.session, i, c.as_mut_ptr()) == 0 {
                let c = c.assume_init();
                if !c.code.is_null() {
                    out.push((text(c.code), c.p));
                }
            }
        }
    }

    Ok(LangGuess { ranked: out, allowed_mass })
}
