use crate::trap::TrapContext;
use crate::config::{
    KERNEL_STACK_SIZE,
    USER_STACK_SIZE,
    APP_SIZE_LIMIT,
    APP_BASE_ADDRESS,
    MAX_APP_NUM,
};

#[derive(Clone, Copy)]
#[repr(align(4096))]
struct KernelStack {
    data: [u8; KERNEL_STACK_SIZE],
}

#[derive(Clone, Copy)]
#[repr(align(4096))]
struct UserStack {
    data: [u8; USER_STACK_SIZE],
}

/// 基础内核栈，处于程序的 `.bss` 段
/// 
/// 当前内核为每个程序设置一个内核栈
static KERNEL_STACK: [KernelStack; MAX_APP_NUM] = [KernelStack {
    data: [0; KERNEL_STACK_SIZE],
}; MAX_APP_NUM];

/// 基础用户栈，处于程序的 `.bss` 段
/// 
/// 当前内核使用数组管理用户栈
static USER_STACK: [UserStack; MAX_APP_NUM] = [UserStack {
    data: [0; USER_STACK_SIZE],
}; MAX_APP_NUM];

impl KernelStack {
    fn get_sp(&self) -> usize {
        self.data.as_ptr() as usize + KERNEL_STACK_SIZE
    }

    pub fn push_app_context(&self, context: TrapContext) -> usize {
        let tcx_ptr = (self.get_sp() - core::mem::size_of::<TrapContext>()) as *mut TrapContext;
        unsafe {
            *tcx_ptr = context;
        }
        tcx_ptr as usize
    }
}

impl UserStack {
    fn get_sp(&self) -> usize {
        self.data.as_ptr() as usize + USER_STACK_SIZE
    }
}

/// 获取用户程序个数
pub fn get_app_num() -> usize {
    unsafe extern "C" {
        safe fn _num_app();
    }
    unsafe { (_num_app as *const usize).read_volatile() }
}

/// 依据用户程序编号返回程序入口地址
pub fn get_app_base(app_id: usize) -> usize {
    APP_BASE_ADDRESS + app_id * APP_SIZE_LIMIT
}

/// 加载用户程序至固定内存地址
/// 
/// 此处各程序的地址不重叠，应该无需添加内存屏障
pub fn load_app() {
    unsafe extern "C" {
        safe fn _num_app();
    }
    let app_ptr = _num_app as *const usize;
    let app_num = unsafe { app_ptr.read_volatile() };
    let app_start = unsafe { core::slice::from_raw_parts(app_ptr.add(1), app_num + 1) };
    for i in 0..app_num {
        let app_base_i = APP_BASE_ADDRESS + i * APP_SIZE_LIMIT;
        (app_base_i..app_base_i + APP_SIZE_LIMIT).for_each(|addr| unsafe {
            (addr as *mut u8).write_volatile(0);
        });
        let app_len = app_start[i+1] - app_start[i];
        let app_src = unsafe {
            core::slice::from_raw_parts(
                app_start[i] as *const u8,
                app_len
            )
        };
        let dst = unsafe { core::slice::from_raw_parts_mut(app_base_i as *mut u8, app_len) };
        dst.copy_from_slice(app_src);
    }
}

/// 初始化用户程序导入上下文，并存入对应内核栈
pub fn init_app_context(app_id: usize) -> usize {
    KERNEL_STACK[app_id].push_app_context(TrapContext::init_app_context(
        get_app_base(app_id), 
        USER_STACK[app_id].get_sp()
    ))
}
