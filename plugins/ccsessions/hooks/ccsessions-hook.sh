#!/bin/sh
# ccsessions の hook ラッパー。
#
# **プラグインだけ入れてバイナリが無い人を壊さないこと**が、このスクリプトの
# 唯一の要件（docs/adr/0021-distribution.md）。プラグインは marketplace から
# 誰でも入れられる一方、`ccsessionsd` / `ccsessions` は brew で別に入れる必要が
# あるので、「プラグインだけある」状態は普通に起きる。その状態は**静かで安全**
# でなければならない。
#
# したがって:
#
# - バイナリが無ければ**何もせず exit 0**。エラーも警告も出さない
#   （hook の失敗は Claude Code 側でユーザに見える形で現れるため）。
# - どの経路を通っても**最後は必ず exit 0**。`ccsessions hook` 自体も exit 0 を
#   保証しているが（ADR 0004）、ここでも二重に担保する。hook が非 0 を返すと
#   権限を拒否したりユーザのプロンプトを消したりしうる。
# - **stdout に何も書かない。** `ccsessions hook` は stdout を汚さない
#   （integration test の hook_always_exits_zero_and_writes_nothing_to_stdout）。
#
# stdin（hook payload の JSON）はそのまま `ccsessions hook` へ渡る。
#
# `exec` を使わないのは、exec に失敗した場合（PATH から消えた等）に sh が
# 非 0 で抜けうるため。1 プロセス増えるが hook は数 ms で終わるので問題ない。

if command -v ccsessions >/dev/null 2>&1; then
	ccsessions hook
fi

exit 0
