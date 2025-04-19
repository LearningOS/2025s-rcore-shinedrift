//! Process management syscalls
use crate::task::{change_program_brk, exit_current_and_run_next, suspend_current_and_run_next, current_user_token};
use crate::timer::get_time_us;
use crate::mm::translated_byte_buffer;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn copy_to_user(user_token: usize, user_ptr: *mut u8, kernel_src: *const u8, len: usize) -> Result<(), ()> {
    let mut buffers = translated_byte_buffer(user_token, user_ptr, len);
    let mut offset = 0;

    for buf in buffers.iter_mut() {
        let copy_len = core::cmp::min(buf.len(), len - offset);
        if len == 0 || copy_len == 0 {
            return Err(());
        }
        unsafe {
            core::ptr::copy_nonoverlapping(kernel_src.add(offset), buf.as_mut_ptr(), copy_len);
        }
        offset += copy_len;
    }

    if offset == len {
        Ok(())
    } else {
        Err(())
    }
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let time_val  = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let user_token = current_user_token(); // 获取当前任务的虚拟内存上下文
    let kernel_src = &time_val as *const TimeVal as *const u8;
    let len = core::mem::size_of::<TimeVal>();

    // 使用封装函数复制数据到用户态
    if copy_to_user(user_token, ts as *mut u8, kernel_src, len).is_ok() {
        0 // 返回成功
    } else {
        -1 // 返回失败
    }
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(_trace_request: usize, _id: usize, _data: usize) -> isize {
    trace!("kernel: sys_trace");
    -1
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    -1
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    -1
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
