//! Привязка дочерних процессов к Windows Job Object с флагом JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
//! Гарантирует, что при завершении (или аварийном падении) родительского процесса ядро Windows
//! автоматически и принудительно завершит все ассоциированные дочерние процессы (audiocpp_server.exe и др.).

#[cfg(windows)]
pub mod sys {
    use std::os::windows::io::RawHandle;
    use std::sync::OnceLock;

    type HANDLE = *mut std::ffi::c_void;
    type BOOL = i32;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[repr(C)]
    #[derive(Default)]
    struct IO_COUNTERS {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        basic_limit_information: JOBOBJECT_BASIC_LIMIT_INFORMATION,
        io_info: IO_COUNTERS,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_limit: usize,
        peak_job_memory_limit: usize,
    }

    extern "system" {
        fn CreateJobObjectW(
            lp_job_attributes: *const std::ffi::c_void,
            lp_name: *const u16,
        ) -> HANDLE;
        fn SetInformationJobObject(
            h_job: HANDLE,
            job_object_information_class: u32,
            lp_job_object_information: *const std::ffi::c_void,
            cb_job_object_information_length: u32,
        ) -> BOOL;
        fn AssignProcessToJobObject(h_job: HANDLE, h_process: HANDLE) -> BOOL;
        fn CloseHandle(h_object: HANDLE) -> BOOL;
    }

    struct SafeJobHandle(HANDLE);
    unsafe impl Send for SafeJobHandle {}
    unsafe impl Sync for SafeJobHandle {}

    impl Drop for SafeJobHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    static GLOBAL_JOB: OnceLock<Option<SafeJobHandle>> = OnceLock::new();

    fn get_global_job() -> Option<HANDLE> {
        GLOBAL_JOB
            .get_or_init(|| unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(job);
                    None
                } else {
                    Some(SafeJobHandle(job))
                }
            })
            .as_ref()
            .map(|j| j.0)
    }

    /// Привязать дочерний процесс к глобальному Job Object Windows.
    pub fn assign_process_to_global_job(process_handle: RawHandle) -> bool {
        if let Some(job) = get_global_job() {
            unsafe { AssignProcessToJobObject(job, process_handle as HANDLE) != 0 }
        } else {
            false
        }
    }
}

#[cfg(windows)]
pub use sys::assign_process_to_global_job;

#[cfg(not(windows))]
pub fn assign_process_to_global_job<T>(_process_handle: T) -> bool {
    true
}
