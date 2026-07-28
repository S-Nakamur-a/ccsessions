# 0011 · メニューバー高が測れないときは 24pt を仮定する

採用 · 2026-07-26

## 文脈

メニューバー高は
`(frame.max_y − visibleFrame.max_y).max(safeAreaInsets.top)` で測っている。

- ノッチ機でメニューバー自動非表示 → `0.max(32)` = 32。`safeAreaInsets` が保険になる
- **非ノッチ画面で自動非表示 → `0.max(0)` = 0**。保険が無い

0 のときにクランプを素通しすると、帯が本来の高さのまま画面最上端に置かれる。
そこはメニューバーではなく**アプリのコンテンツ領域**なので、幅 × 高さぶんの
クリックを奪う。フルスクリーン利用中に起こりうる。

## 決定

`geometry::bar_rect` は、測った高さが 0 のとき `layout::FALLBACK_MENU_BAR_H = 24.0`
でクランプする（Big Sur 以降の標準的な高さ）。

## 理由

- **測れないときは小さい方へ倒す。** 大きい方に倒すのは危険側で、そのままクリックを
  奪う事故になる。
- 下限を `menu_bar_height()` 側に入れる案は採らなかった。「測れなかった」と
  「本当に 0」を区別できなくなるため、扱いは `bar_rect` 側に置く。

## 影響

番人は `layout.rs::an_unmeasurable_menu_bar_falls_back_to_a_sane_height` と
`geometry.rs::band_is_clipped_to_the_menu_bar_and_sits_at_the_top`。
