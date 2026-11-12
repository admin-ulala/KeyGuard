//! 256 位按键掩码，按 VK 码索引。
//!
//! 热路径只做一次 `load` + 移位判断；写入只在「按下 / 抬起」时发生，频率等于人手敲键频率。

use std::sync::atomic::{AtomicU64, Ordering};

pub struct KeyMask([AtomicU64; 4]);

impl Default for KeyMask {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyMask {
    pub const fn new() -> Self {
        Self([
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
        ])
    }

    #[inline(always)]
    pub fn get(&self, vk: u32) -> bool {
        let idx = (vk >> 6) as usize & 3;
        (self.0[idx].load(Ordering::Relaxed) >> (vk & 63)) & 1 == 1
    }

    #[inline(always)]
    pub fn set(&self, vk: u32) {
        let idx = (vk >> 6) as usize & 3;
        self.0[idx].fetch_or(1u64 << (vk & 63), Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn clear(&self, vk: u32) {
        let idx = (vk >> 6) as usize & 3;
        self.0[idx].fetch_and(!(1u64 << (vk & 63)), Ordering::Relaxed);
    }

    pub fn clear_all(&self) {
        for w in &self.0 {
            w.store(0, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> [u64; 4] {
        [
            self.0[0].load(Ordering::Relaxed),
            self.0[1].load(Ordering::Relaxed),
            self.0[2].load(Ordering::Relaxed),
            self.0[3].load(Ordering::Relaxed),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear() {
        let m = KeyMask::new();
        assert!(!m.get(0x5B));
        m.set(0x5B);
        m.set(0x00); // 边界：最低位
        m.set(0xFF); // 边界：最高字节
        assert!(m.get(0x5B));
        assert!(m.get(0x00));
        assert!(m.get(0xFF));
        assert!(!m.get(0x5C));
        m.clear(0x5B);
        assert!(!m.get(0x5B));
        assert!(m.get(0xFF));
        m.clear_all();
        assert_eq!(m.snapshot(), [0, 0, 0, 0]);
    }
}
