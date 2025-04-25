//! Process management syscalls
use alloc::sync::Arc;

use crate::{
    loader::get_app_data_by_name,
    mm::{translated_refmut, translated_str, translated_byte_buffer},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next, 
    },
    timer::get_time_us,
    mm::{MapPermission, VPNRange, VirtAddr, PageTable}
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

///
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

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
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

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_mmap NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    let current_task = current_task().unwrap();
    let start_va = VirtAddr(start);
    if !start_va.aligned() {
        return -1;
    }
    if len <= 0 {
        return -1;
    }
    if (prot & !0x7 != 0) || (prot & 0x7 == 0) {
        return -1;
    }
    // len按页向上取整
    let start_va = VirtAddr(start).floor();
    let end_va = VirtAddr(start + len).ceil();
    let pgtb  = PageTable::from_token(current_user_token());

    // 检查虚拟地址是否已经被映射
    let vpns = VPNRange::new(start_va, end_va);
    for vpn in vpns {
        if let Some(pte) = pgtb.translate(vpn) {
            if pte.is_valid() {
                return -1;
            }
        }
    };

    // 构造permission: MapPermission
    let mut perm: MapPermission = MapPermission::U;
    if prot & 0x1 != 0 {
        perm |= MapPermission::R;
    }
    if prot & 0x2 != 0 {
        perm |= MapPermission::W;
    }
    if prot & 0x4 != 0 {
        perm |= MapPermission::X;
    }
    current_task.mmap(start_va.into(), end_va.into(), perm);
    0
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_munmap NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    // 处理参数，获取需要取消映射的区间
    let start_va = VirtAddr(start);
    if !start_va.aligned() {
        return -1;
    }
    if len <= 0 {
        return -1;
    }
    let current_task = current_task().unwrap();
    let start_vpn = start_va.floor();
    let end_vpn = VirtAddr(start + len).ceil();
    current_task.munmap(start_vpn.into(), end_vpn.into())
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(_path: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_spawn NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    -1
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(_prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    -1
}
