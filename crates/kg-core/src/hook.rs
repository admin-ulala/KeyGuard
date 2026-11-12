//! 低级键盘钩子（`WH_KEYBOARD_LL`）。
//!
//! 结构：一个专用线程装钩子并跑消息循环；回调直接读 `Shared` 的原子量做判定。
//! 回调返回 1 = 吞掉该事件，全系统（包括独占全屏游戏）都收不到。
//!
//! 热路径只有三步：读原子 → 扫槽位表（≤160 条）→ 返回。没有分配、没有锁、没有 IO。
//! 诊断采样（QPC 两次调用）也在 0.05µs 量级，相对 200µs 预算可忽略。

use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{mpsc, Arc};

use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
};

use crate::state::*;
use crate::vk;
use crate::win;

const WM_KEYDOWN: usize = 0x0100;
const WM_SYSKEYDOWN: usize = 0x0104; // Alt 组合键走这条；漏了它 Alt+Tab 永远拦不住
const LLKHF_INJECTED: u32 = 0x10; // 模拟输入标记，见到就放行

const M_ALT: u8 = crate::catalog::M_ALT;
const M_CTRL: u8 = crate::catalog::M_CTRL;
const M_SHIFT: u8 = crate::catalog::M_SHIFT;

/// 把「生效掩码」翻译成回调要扫的槽位表。
///
/// 槽位打包成 u32：低 8 位 vk，8..16 位 need。VK 全在 0..255，够用。
/// 先写槽位，最后用 Release 发布 hot_n：并发时最坏只是少拦一条，
/// 不会读到半条垃圾规则。
///
/// 布局：内置条目先压，自定义条目紧接着压（紧排，回调只扫 `0..hot_n`，没有空洞）。
/// 自定义一律按 `Hook` 语义处理 —— 用户加进来就是要它真的被吞掉。
pub fn rebuild(st: &Shared) {
    let mask = st.hotkeys_eff();
    let custom_mask = st.custom_eff();
    let mut n = 0usize;
    'outer: for (i, e) in crate::catalog::ENTRIES.iter().enumerate() {
        if i >= 64 || mask & (1 << i) == 0 {
            continue;
        }
        if e.how != crate::catalog::How::Hook {
            continue;
        }
        for &vk in e.vks {
            if n >= st.hot_slots.len() {
                break 'outer;
            }
            st.hot_slots[n].store((vk & 0xFF) | ((e.need as u32) << 8), Ordering::Relaxed);
            n += 1;
        }
    }
    // 自定义条目：rebuild 不在热路径（回调只读 hot_slots / hot_n），读一次规则锁可以接受
    if custom_mask != 0 {
        if let Ok(rs) = st.rules.read() {
            'custom: for (i, c) in rs.custom.iter().enumerate().take(64) {
                if custom_mask & (1 << i) == 0 {
                    continue;
                }
                for &vk in &c.vks {
                    if n >= st.hot_slots.len() {
                        break 'custom;
                    }
                    st.hot_slots[n]
                        .store((vk & 0xFF) | ((c.need as u32) << 8), Ordering::Relaxed);
                    n += 1;
                }
            }
        }
    }
    st.hot_n.store(n as u32, Ordering::Release);
    st.hot_rev.fetch_add(1, Ordering::Relaxed);
}

/// 全局状态指针。只用一次，进程生命周期内不变。
static STATE: AtomicPtr<Shared> = AtomicPtr::new(std::ptr::null_mut());

#[inline(always)]
fn should_block(st: &Shared, vk: u32) -> bool {
    let n = st.hot_n.load(Ordering::Acquire) as usize;
    if n == 0 {
        return false;
    }
    let alt = st.alt.load(Ordering::Relaxed);
    let ctrl = st.ctrl.load(Ordering::Relaxed);
    let shift = st.shift.load(Ordering::Relaxed);

    for i in 0..n {
        let d = st.hot_slots[i].load(Ordering::Relaxed);
        if d & 0xFF != vk & 0xFF {
            continue;
        }
        let need = ((d >> 8) & 0xFF) as u8;
        if need & M_ALT != 0 && !alt {
            continue;
        }
        if need & M_CTRL != 0 && !ctrl {
            continue;
        }
        if need & M_SHIFT != 0 && !shift {
            continue;
        }
        return true;
    }
    false
}

/// 处理一个键盘事件，返回 true 表示吞掉。
///
/// 单独成函数，是为了让「统计」只写一份：以前每个 `return 1` 都要自己补计数，
/// 加一处分支就漏一处。
unsafe fn handle(st: &Shared, kb: &KBDLLHOOKSTRUCT, wparam: WPARAM) -> bool {
    let vk = kb.vkCode;
    let down = wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN;

    // 一键解除：永远放行，且不做任何记录
    if vk == st.panic_vk.load(Ordering::Relaxed) {
        let alt_ok = !st.panic_need_alt.load(Ordering::Relaxed) || st.alt.load(Ordering::Relaxed);
        let ctrl_ok = !st.panic_need_ctrl.load(Ordering::Relaxed) || st.ctrl.load(Ordering::Relaxed);
        if alt_ok && ctrl_ok {
            if down && !st.panic_held.swap(true, Ordering::Relaxed) {
                st.toggle();
            }
            if !down {
                st.panic_held.store(false, Ordering::Relaxed);
            }
            return false;
        }
    }

    // 低位钩子可能给出通用码，也可能给出左右分码（keybd_event / 部分键盘），两种都要认
    match vk {
        vk::VK_MENU | vk::VK_LMENU | vk::VK_RMENU => st.alt.store(down, Ordering::Relaxed),
        vk::VK_CONTROL | vk::VK_LCONTROL | vk::VK_RCONTROL => st.ctrl.store(down, Ordering::Relaxed),
        vk::VK_LSHIFT | vk::VK_RSHIFT => st.shift.store(down, Ordering::Relaxed),
        _ => {}
    }

    if st.is_armed() {
        if should_block(st, vk) {
            if down {
                st.held.set(vk);
                // 只在按下计数：「拦截 N 次」= 用户按了 N 次，不是 2N 次
                st.stat_blocked.fetch_add(1, Ordering::Relaxed);
                st.per_vk[(vk as usize) & 0xFF].fetch_add(1, Ordering::Relaxed);
            } else {
                st.held.clear(vk);
            }
            return true;
        }
        // 按下被吞过、抬起时修饰键已经松开：抬起也要吞，避免目标程序卡键。
        // 但这一次不算「拦截」，否则计数就翻倍了。
        if !down && st.held.get(vk) {
            st.held.clear(vk);
            return true;
        }
    }
    if down {
        st.stat_passed.fetch_add(1, Ordering::Relaxed);
    }
    false
}

unsafe extern "system" fn kb_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let p = STATE.load(Ordering::Acquire);
        if !p.is_null() {
            let st = &*p;
            let kb = &*(lparam as *const KBDLLHOOKSTRUCT);

            let injected = (kb.flags & LLKHF_INJECTED) != 0;
            if !injected || !st.ignore_injected.load(Ordering::Relaxed) {
                // 只在按下采样：hook_samples 要能对上「按了多少次键」，抬起不再翻倍
                let down = wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN;
                let t0 = if down && st.profiling.load(Ordering::Relaxed) {
                    win::ticks()
                } else {
                    0
                };
                let block = handle(st, kb, wparam);
                if t0 != 0 {
                    st.hist_sample(win::ticks_to_us(t0));
                }
                if block {
                    return 1;
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub struct HookHandle {
    _state: Arc<Shared>,
    _thread: std::thread::JoinHandle<()>,
}

/// 装钩子。返回时钩子已经生效（或明确失败），不会出现「以为装上了其实没有」。
pub fn install(state: Arc<Shared>) -> Result<HookHandle, u32> {
    // 泄漏一个强引用给全局指针：进程活多久它活多久，回调里永远不会踩到已释放内存
    let raw = Arc::into_raw(state.clone()) as *mut Shared;
    STATE.store(raw, Ordering::Release);

    // 装钩子前先把槽位表建好，否则装好瞬间表是空的
    rebuild(&state);

    let (tx, rx) = mpsc::channel::<Result<(), u32>>();
    let thread = std::thread::spawn(move || unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), std::ptr::null_mut(), 0);
        if hook.is_null() {
            let _ = tx.send(Err(GetLastError()));
            return;
        }
        let _ = tx.send(Ok(()));

        // 钩子回调在装钩子的这个线程上派发，必须跑消息循环，否则回调永远不来
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(hook);
    });

    match rx.recv() {
        Ok(Ok(())) => Ok(HookHandle {
            _state: state,
            _thread: thread,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(0),
    }
}
