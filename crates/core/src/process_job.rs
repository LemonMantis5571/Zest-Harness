//! Process-tree kill for commands Zest spawned.
//!
//! `TerminateProcess` on `cmd.exe` leaves its children running. Windows job
//! objects are the kernel's tree: children started after assignment die with
//! the job. Unix already uses a process group.

use std::process::Stdio;

/// Job that owns a spawned command and every child it starts afterwards.
pub struct ProcessJob {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for ProcessJob {}
#[cfg(windows)]
unsafe impl Sync for ProcessJob {}

impl ProcessJob {
    pub fn new() -> Option<Self> {
        #[cfg(windows)]
        {
            windows_job()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn assign(&self, pid: u32) -> bool {
        #[cfg(windows)]
        {
            assign_pid(self.handle, pid)
        }
        #[cfg(not(windows))]
        {
            let _ = pid;
            false
        }
    }

    pub fn terminate(&self) {
        #[cfg(windows)]
        {
            terminate_job(self.handle);
        }
    }
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            close_handle(self.handle);
        }
    }
}

/// Flags for a GUI-hosted command that must not flash a console, and that
/// stays suspended until it is in a job — otherwise `cmd` can start `ping`
/// before assignment and that child survives the timeout.
#[cfg(windows)]
pub fn windows_creation_flags() -> u32 {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    CREATE_NO_WINDOW | CREATE_SUSPENDED
}

#[cfg(windows)]
pub fn resume_process(pid: u32) {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if !valid_handle(snapshot) {
        return;
    }

    let mut entry = unsafe { std::mem::zeroed::<THREADENTRY32>() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut ok = unsafe { Thread32First(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if valid_handle(thread) {
                unsafe {
                    let _ = ResumeThread(thread);
                    let _ = CloseHandle(thread);
                }
            }
        }
        ok = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
}

pub fn terminate_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let process_group = format!("-{pid}");
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &process_group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(windows)]
fn windows_job() -> Option<ProcessJob> {
    use std::mem::size_of;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if !valid_handle(handle) {
        return None;
    }

    let mut info = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ok = unsafe {
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        close_handle(handle);
        return None;
    }
    Some(ProcessJob { handle })
}

#[cfg(windows)]
fn assign_pid(job: windows_sys::Win32::Foundation::HANDLE, pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
    if !valid_handle(process) {
        return false;
    }
    let ok = unsafe { AssignProcessToJobObject(job, process) };
    unsafe {
        let _ = CloseHandle(process);
    }
    ok != 0
}

#[cfg(windows)]
fn terminate_job(handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    unsafe {
        let _ = TerminateJobObject(handle, 1);
    }
}

#[cfg(windows)]
fn close_handle(handle: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Foundation::CloseHandle;
    if valid_handle(handle) {
        unsafe {
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(windows)]
fn valid_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> bool {
    !handle.is_null() && handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
}
