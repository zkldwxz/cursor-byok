# 全新应用图标（BYOK key-caret）

_Written by plan mode · 2026-10-21T07:14:16.691Z · cc-switch-zhipu-glm/glm-5.3_

## Steps

1. 创建新 icon 的 SVG 设计源
   - apps/desktop/src-tauri/icons/icon.svg：1024×1024，暖炭黑圆角底，米白块状文字光标上用负空间冲出锁孔，锁孔后透黄铜光。这是设计源文件，作为重新生成的唯一来源。
2. 渲染 1024 PNG 并重新生成全套图标
   - 在 /tmp 用 scratch 安装 sharp 渲染 icon.svg → 1024×1024 PNG（带透明圆角），然后 `npm --prefix apps/desktop run tauri icon <png>` 重新生成 icons/ 下全部尺寸（icns/ico/png）。
3. 校验生成结果
   - npm --prefix apps/desktop run check 通过；用 sips/file 确认各尺寸像素正确、icns/ico 均已更新。cargo 未触碰，无需重跑。

## Notes

设计概念（v3 定稿）：块状光标 + 锁孔。米白块状文字光标（编辑器原生形状）上冲出一个锁孔，锁孔后透出暖黄铜光——光标是 Cursor 的世界，锁孔是「自带钥匙」。配色全部取自 Cursor UI 实测：暖炭黑 #1A1711→#0D0C09、面板描边 #4A4438、米白 #FFFDF8→#E6E2D8，唯一彩色是黄铜 #F8CF8C→#CC8A41。

已否决：v1 艳蓝渐变 + 终端符 + 钥匙（两个符号拼接，不伦不类）；v2 胶囊光标 + 锁孔（比例像胶囊按钮）。

- `cursor.svg`（应用内图标）不动——它标注 Cursor 编辑器分区，非应用标识。
- 圆角与透明边角烘焙在 SVG 中（rx 224），macOS 下不需系统再切。
- 小尺寸已验证：32/64/128 并排可辨（白色方块 + 黄铜标记）。
- tauri icon 会覆盖 icons/ 下全部现有文件，git diff 可完整审查。
