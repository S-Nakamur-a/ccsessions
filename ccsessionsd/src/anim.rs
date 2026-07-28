//! CoreAnimation のアニメーション組み立て。
//!
//! **なぜ CoreAnimation に任せるか**: アニメはレンダーサーバ側で無限ループするため、
//! daemon 側のスレッドは寝たままでよい（タイマで毎フレーム叩かない）。常駐アプリの
//! 省電力性はここで決まる（実測でアイドル時 CPU 0.0–0.1%）。
//!
//! すべてのアニメは**同じキーで貼り直すと前のアニメが置換される**。状態が変わった
//! ときだけ貼り直し、変わっていなければ触らない（触ると位相がリセットされ、群れの
//! 動きが不自然に揃ってしまう）。

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSNumber};
use objc2_quartz_core::{
    kCAMediaTimingFunctionEaseInEaseOut, CAAnimation, CAAnimationGroup, CABasicAnimation,
    CAKeyframeAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction,
};

use crate::ffi::ns;

/// 往復アニメ（`autoreverses`）を作る。CSS の `0% → 50% → 100%` で戻る形に対応する。
/// `half_secs` は片道の秒数（CSS の全周期の半分）。
fn basic_autoreverse(
    key_path: &str,
    from: f64,
    to: f64,
    half_secs: f64,
) -> Retained<CABasicAnimation> {
    let anim = CABasicAnimation::animationWithKeyPath(Some(&ns(key_path)));
    let f = NSNumber::numberWithDouble(from);
    let t = NSNumber::numberWithDouble(to);
    // SAFETY: スカラの keyPath（translation / opacity / shadowRadius）に NSNumber を渡す。
    unsafe {
        anim.setFromValue(Some(f.as_ref() as &AnyObject));
        anim.setToValue(Some(t.as_ref() as &AnyObject));
    }
    // SAFETY: kCAMediaTimingFunctionEaseInEaseOut は QuartzCore の extern static。
    let ease =
        CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionEaseInEaseOut });
    anim.setTimingFunction(Some(&ease));
    anim.setDuration(half_secs);
    anim.setAutoreverses(true);
    anim.setRepeatCount(f32::INFINITY);
    anim
}

/// キーフレームアニメを作る（`hop` や `eyeblink` のような非対称な動き）。
fn keyframe(
    key_path: &str,
    values: &[f64],
    keys: &[f64],
    secs: f64,
) -> Retained<CAKeyframeAnimation> {
    let anim = CAKeyframeAnimation::animationWithKeyPath(Some(&ns(key_path)));

    let vals: Vec<Retained<NSNumber>> = values
        .iter()
        .map(|&v| NSNumber::numberWithDouble(v))
        .collect();
    let val_objs: Vec<&AnyObject> = vals.iter().map(|v| v.as_ref() as &AnyObject).collect();
    // SAFETY: スカラの keyPath に NSNumber 配列を渡す。要素型は keyPath に一致。
    unsafe { anim.setValues(Some(&NSArray::from_slice(&val_objs))) };

    let ks: Vec<Retained<NSNumber>> = keys
        .iter()
        .map(|&k| NSNumber::numberWithDouble(k))
        .collect();
    let key_refs: Vec<&NSNumber> = ks.iter().map(|k| k.as_ref()).collect();
    anim.setKeyTimes(Some(&NSArray::from_slice(&key_refs)));

    anim.setDuration(secs);
    anim.setRepeatCount(f32::INFINITY);
    anim
}

/// 体の上下揺れ（作業中の `bob`）。CALayer は y 上向きなので `amp` は正で「上へ」。
pub fn bob(layer: &CALayer, amp: f64, half_secs: f64) {
    let a = basic_autoreverse("transform.translation.y", 0.0, amp, half_secs);
    layer.addAnimation_forKey(&a, Some(&ns(MOVE_KEY)));
}

/// 体の横漂い（エージェント待ちの `drift`）。
pub fn drift(layer: &CALayer, amp: f64, half_secs: f64) {
    let a = basic_autoreverse("transform.translation.x", 0.0, amp, half_secs);
    layer.addAnimation_forKey(&a, Some(&ns(MOVE_KEY)));
}

/// 体の跳ね（判断待ちの `hop`）。着地でわずかに沈む 2 段モーション。
pub fn hop(layer: &CALayer, keys: &[f64], values: &[f64], secs: f64) {
    let a = keyframe("transform.translation.y", values, keys, secs);
    layer.addAnimation_forKey(&a, Some(&ns(MOVE_KEY)));
}

/// エラーのゆっくり明滅（`errBreath`）。
///
/// 元デザインは `filter: brightness()` と `box-shadow` を動かすが、CALayer に
/// brightness は無い。**明度の代わりに全体の opacity、box-shadow の代わりに
/// shadowRadius** を往復させる。狙い（グリッチではない、目に優しい呼吸）は保たれる。
pub fn breath(
    face: &CALayer,
    body: &CALayer,
    opacity: (f32, f32),
    glow: (f64, f64),
    half_secs: f64,
) {
    let o = basic_autoreverse("opacity", opacity.0 as f64, opacity.1 as f64, half_secs);
    face.addAnimation_forKey(&o, Some(&ns(MOVE_KEY)));
    let g = basic_autoreverse("shadowRadius", glow.0, glow.1, half_secs);
    body.addAnimation_forKey(&g, Some(&ns(GLOW_KEY)));
}

/// 瞬き（作業中の目）。一瞬だけ縦に潰す。
pub fn blink(layer: &CALayer, keys: &[f64], values: &[f64], secs: f64) {
    let a = keyframe("transform.scale.y", values, keys, secs);
    layer.addAnimation_forKey(&a, Some(&ns(BLINK_KEY)));
}

/// 吹き出しの上下揺れ（判断待ちの `!`）。
pub fn bubble_pop(layer: &CALayer, amp: f64, half_secs: f64) {
    let a = basic_autoreverse("transform.translation.y", 0.0, amp, half_secs);
    layer.addAnimation_forKey(&a, Some(&ns(POP_KEY)));
}

/// `z` の浮遊（アイドル）。移動・拡大・フェードを 1 グループで同期させる。
///
/// 3 本を別々に貼ると `repeatCount` の端で位相がずれていくので、`CAAnimationGroup`
/// でまとめて 1 つの周期に閉じ込める。
pub fn float_z(
    layer: &CALayer,
    to: (f64, f64),
    scale: (f64, f64),
    op_keys: &[f64],
    op_values: &[f32],
    secs: f64,
) {
    let mx = keyframe("transform.translation.x", &[0.0, to.0], &[0.0, 1.0], secs);
    let my = keyframe("transform.translation.y", &[0.0, to.1], &[0.0, 1.0], secs);
    let sc = keyframe("transform.scale", &[scale.0, scale.1], &[0.0, 1.0], secs);
    let ops: Vec<f64> = op_values.iter().map(|&v| v as f64).collect();
    let op = keyframe("opacity", &ops, op_keys, secs);

    let group = CAAnimationGroup::new();
    let anims: Vec<&CAAnimation> = vec![&mx, &my, &sc, &op];
    group.setAnimations(Some(&NSArray::from_slice(&anims)));
    group.setDuration(secs);
    group.setRepeatCount(f32::INFINITY);
    layer.addAnimation_forKey(&group, Some(&ns(FLOAT_KEY)));
}

/// カード内 agent ドットの小刻みな震え（作業中）。
pub fn jitter(layer: &CALayer, amp: f64, secs: f64) {
    let a = keyframe(
        "transform.translation.x",
        &[0.0, amp, -amp, amp, 0.0],
        &[0.0, 0.25, 0.5, 0.75, 1.0],
        secs,
    );
    layer.addAnimation_forKey(&a, Some(&ns(MOVE_KEY)));
}

/// カード内 agent ドットのゆっくり明滅（エラー）。
pub fn soft_pulse(layer: &CALayer, opacity: (f32, f32), half_secs: f64) {
    let a = basic_autoreverse("opacity", opacity.0 as f64, opacity.1 as f64, half_secs);
    layer.addAnimation_forKey(&a, Some(&ns(MOVE_KEY)));
}

// アニメーションキー。同じキーで貼り直すと置換される。
pub const MOVE_KEY: &str = "cc.move";
pub const GLOW_KEY: &str = "cc.glow";
pub const BLINK_KEY: &str = "cc.blink";
pub const POP_KEY: &str = "cc.pop";
pub const FLOAT_KEY: &str = "cc.float";

/// レイヤに貼ってあるアニメを全部剥がす（`reduce_motion` と状態遷移で使う）。
pub fn clear(layer: &CALayer) {
    for k in [MOVE_KEY, GLOW_KEY, BLINK_KEY, POP_KEY, FLOAT_KEY] {
        layer.removeAnimationForKey(&ns(k));
    }
}
