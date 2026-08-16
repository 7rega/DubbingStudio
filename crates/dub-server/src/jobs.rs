//! Каркас очереди джобов: ОДИН воркер обрабатывает GPU-задачи строго последовательно (GPU не трогается
//! конкурентно — как в backend/app.py). Прогресс стримится по SSE; результат доставляется через oneshot.
//!
//! Формат SSE-событий повторяет app.py: {"type":"progress", ...}, {"type":"done","result":...},
//! {"type":"error","error":"..."}. Терминальная джоба реапится по таймеру, если SSE так и не открыли.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, Mutex};

/// Функция джобы: получает колбэк прогресса (msg + произвольные поля), возвращает JSON-результат.
pub type JobFn = Box<dyn FnOnce(ProgressFn) -> Result<Value, String> + Send + 'static>;

/// Короткий (12 симв.) идентификатор джобы из UUIDv4.
fn new_job_id() -> String {
    let mut id = uuid::Uuid::new_v4().simple().to_string();
    id.truncate(12);
    id
}

/// Колбэк прогресса, передаваемый в тело джобы. Кладёт {"type":"progress", ...} в SSE-канал.
pub type ProgressFn = Arc<dyn Fn(Value) + Send + Sync + 'static>;

/// Пометить в SSE, что стадия взята из чекпоинта (#80): {"type":"progress","stage":..,"resumed":true,..}.
/// Тонкая обёртка над ProgressFn — не меняет очередь/воркер; тело джобы (analyze/render) может звать её,
/// чтобы UI показал «возобновлено из кэша». Фронт, не знающий про поле resumed, его игнорирует.
#[allow(dead_code)] // публичный хелпер для тел джоб (lib.rs — другой агент); analyze шлёт resumed сам.
pub fn emit_resumed(progress: &ProgressFn, stage: &str, msg: &str) {
    progress(json!({ "stage": stage, "msg": msg, "resumed": true }));
}

struct Job {
    tx: broadcast::Sender<Value>, // SSE-события
    status: JobStatus,
    result: Option<Value>,
    error: Option<String>,
    abandoned: bool,
    result_sender: Option<oneshot::Sender<Result<Value, String>>>, // разбудить ожидающего preview/original
}

#[derive(Clone, Copy, PartialEq)]
enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
}

#[derive(Clone)]
pub struct JobQueue {
    inner: Arc<Mutex<HashMap<String, Job>>>,
    submit_tx: tokio::sync::mpsc::Sender<(String, JobFn)>,
}

impl JobQueue {
    /// Создать очередь и запустить единственный воркер.
    pub fn new() -> Self {
        let inner: Arc<Mutex<HashMap<String, Job>>> = Arc::new(Mutex::new(HashMap::new()));
        let (submit_tx, mut submit_rx) =
            tokio::sync::mpsc::channel::<(String, JobFn)>(64);

        let worker_inner = inner.clone();
        tokio::spawn(async move {
            while let Some((job_id, fn_)) = submit_rx.recv().await {
                // Клиент мог отказаться (preview/original по таймауту) -> не запускаем работу.
                {
                    let mut map = worker_inner.lock().await;
                    match map.get_mut(&job_id) {
                        None => continue,
                        Some(j) if j.abandoned => {
                            map.remove(&job_id);
                            continue;
                        }
                        Some(j) => j.status = JobStatus::Running,
                    }
                }

                let tx = {
                    let map = worker_inner.lock().await;
                    map.get(&job_id).map(|j| j.tx.clone())
                };
                let Some(tx) = tx else { continue };

                // Колбэк прогресса: шлём в broadcast (SSE). Ошибку отправки (нет подписчиков) глотаем.
                let tx_p = tx.clone();
                let progress: ProgressFn = Arc::new(move |ev: Value| {
                    let mut obj = match ev {
                        Value::Object(m) => m,
                        other => {
                            let mut m = serde_json::Map::new();
                            m.insert("msg".into(), other);
                            m
                        }
                    };
                    obj.insert("type".into(), json!("progress"));
                    let _ = tx_p.send(Value::Object(obj));
                });

                // Тело джобы синхронное и тяжёлое -> в блокирующий пул.
                let res = tokio::task::spawn_blocking(move || fn_(progress))
                    .await
                    .unwrap_or_else(|e| Err(format!("job panicked: {e}")));

                let mut map = worker_inner.lock().await;
                if let Some(j) = map.get_mut(&job_id) {
                    match &res {
                        Ok(v) => {
                            j.status = JobStatus::Done;
                            j.result = Some(v.clone());
                            let _ = tx.send(json!({"type":"done","result": v}));
                        }
                        Err(e) => {
                            j.status = JobStatus::Error;
                            j.error = Some(e.clone());
                            let _ = tx.send(json!({"type":"error","error": e}));
                        }
                    }
                    // Разбудить ожидающего preview/original.
                    if let Some(sender) = j.result_sender.take() {
                        let _ = sender.send(res); // последнее использование res -> move, без clone
                    }
                }

                // Реап терминальной джобы через 300с, если SSE так и не открыли.
                let reap_inner = worker_inner.clone();
                let reap_id = job_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(300)).await;
                    reap_inner.lock().await.remove(&reap_id);
                });
            }
        });

        JobQueue { inner, submit_tx }
    }

    /// Поставить джобу в очередь; вернуть job_id.
    pub async fn enqueue(&self, fn_: JobFn) -> String {
        let job_id = new_job_id();
        let (tx, _rx) = broadcast::channel(256);
        let job = Job {
            tx,
            status: JobStatus::Queued,
            result: None,
            error: None,
            abandoned: false,
            result_sender: None,
        };
        self.inner.lock().await.insert(job_id.clone(), job);
        let _ = self.submit_tx.send((job_id.clone(), fn_)).await;
        job_id
    }

    /// Поставить джобу и получить oneshot-приёмник результата (для синхронного ожидания preview/original).
    pub async fn enqueue_awaitable(
        &self,
        fn_: JobFn,
    ) -> (String, oneshot::Receiver<Result<Value, String>>) {
        let job_id = new_job_id();
        let (tx, _rx) = broadcast::channel(256);
        let (res_tx, res_rx) = oneshot::channel();
        let job = Job {
            tx,
            status: JobStatus::Queued,
            result: None,
            error: None,
            abandoned: false,
            result_sender: Some(res_tx),
        };
        self.inner.lock().await.insert(job_id.clone(), job);
        let _ = self.submit_tx.send((job_id.clone(), fn_)).await;
        (job_id, res_rx)
    }

    /// Подписаться на SSE-события джобы. Возвращает (receiver, terminal-снапшот если уже завершена).
    pub async fn subscribe(
        &self,
        job_id: &str,
    ) -> Option<(broadcast::Receiver<Value>, Option<Value>)> {
        let map = self.inner.lock().await;
        let job = map.get(job_id)?;
        let rx = job.tx.subscribe();
        // Если джоба уже терминальна, отдать финальное событие сразу (подписка могла опоздать).
        let terminal = match job.status {
            JobStatus::Done => Some(json!({"type":"done","result": job.result})),
            JobStatus::Error => {
                Some(json!({"type":"error","error": job.error.clone().unwrap_or_default()}))
            }
            _ => None,
        };
        Some((rx, terminal))
    }

    pub async fn mark_abandoned(&self, job_id: &str) {
        if let Some(j) = self.inner.lock().await.get_mut(job_id) {
            j.abandoned = true;
        }
    }

    pub async fn remove(&self, job_id: &str) {
        self.inner.lock().await.remove(job_id);
    }

    pub async fn exists(&self, job_id: &str) -> bool {
        self.inner.lock().await.contains_key(job_id)
    }

    pub async fn is_terminal(&self, job_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .get(job_id)
            .map(|j| matches!(j.status, JobStatus::Done | JobStatus::Error))
            .unwrap_or(false)
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}
