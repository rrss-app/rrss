//! 推荐里评测会改动的参数，收成一份。默认值就是生产值：生产构建里 [`current`] 直接返回
//! [`Tuning::DEFAULT`]；评测与单测用 [`with`] 在当前线程换一份，结束或 unwind 时恢复。
//! 没有评测在改的常量仍留在各自的代码旁边。用户可切换的开关在 `store::Settings`。

use crate::freshness::Policy;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// 手调五项的每单位分值：题材特征、源偏好、多源报道、时效惩罚、向量相似，见 `combine::WEIGHTS`。
    /// 拟合模型的先验固定取 `combine::WEIGHTS`，不跟这里变。
    pub weights: [f64; 5],
    /// 时效策略；`None` 按拟合开关选：拟合用按发文频率的保质期，手调用固定 3 天半衰期。
    pub freshness: Option<Policy>,
    /// 拟合：各源效应向先验收缩的强度。
    pub source_penalty: f64,
    /// 拟合：源效应每天按条件标准差抽样的（倍数, 标准差上限）；倍数为 0 不抽样。
    pub source_draw: (f64, f64),
    /// 拟合：训练位置分 0–2 / 3–9 / 10–29 / 30+；`false` 退回 0–9 合成一档。
    pub fine_positions: bool,
    /// 排列：同源扣分往前看几张卡片。
    pub same_feed_window: usize,
    /// 排列：次兴趣校准（题材的第三条入选通道与列表校准）。
    pub topic_calibration: bool,
    /// 拟合开启时每天一个探索位。
    pub exploration: bool,
}

impl Tuning {
    pub const DEFAULT: Self = Self {
        weights: crate::combine::WEIGHTS,
        freshness: None,
        source_penalty: crate::model::LAMBDA,
        source_draw: crate::model::SOURCE_DRAW,
        fine_positions: true,
        same_feed_window: crate::page::SAME_FEED_WINDOW,
        topic_calibration: true,
        exploration: true,
    };
}

impl Default for Tuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(any(test, feature = "evaluation-clock"))]
thread_local! {
    static CURRENT: std::cell::Cell<Tuning> = const { std::cell::Cell::new(Tuning::DEFAULT) };
}

/// 当前线程生效的参数。
pub fn current() -> Tuning {
    #[cfg(any(test, feature = "evaluation-clock"))]
    return CURRENT.get();
    #[cfg(not(any(test, feature = "evaluation-clock")))]
    Tuning::DEFAULT
}

/// 评测：在当前线程用 `tuning` 运行 `f`，结束或 unwind 时恢复原来的参数；不影响其他线程。
#[cfg(any(test, feature = "evaluation-clock"))]
pub fn with<T>(tuning: Tuning, f: impl FnOnce() -> T) -> T {
    struct Restore(Tuning);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.set(self.0);
        }
    }
    let _restore = Restore(CURRENT.replace(tuning));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_overrides_restore_on_return_unwind_and_stay_on_their_thread() {
        let window = Tuning {
            same_feed_window: 4,
            ..Tuning::DEFAULT
        };
        with(window, || {
            assert_eq!(current().same_feed_window, 4);
            with(
                Tuning {
                    exploration: false,
                    ..current()
                },
                || {
                    assert_eq!(
                        (current().same_feed_window, current().exploration),
                        (4, false)
                    );
                },
            );
            assert!(current().exploration);
            let _ = std::panic::catch_unwind(|| with(Tuning::DEFAULT, || panic!("unwind")));
            assert_eq!(current(), window);
            std::thread::spawn(|| assert_eq!(current(), Tuning::DEFAULT))
                .join()
                .unwrap();
        });
        assert_eq!(current(), Tuning::DEFAULT);
    }
}
