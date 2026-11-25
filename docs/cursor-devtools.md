# Cursor DevTools 与调用统计

BYOK 会自动在 Cursor 会话中提供 `cursor-devtools` MCP。Chrome 配套扩展负责浏览器调试，安装在使用 Cursor 的桌面。扩展独立维护和分发。

```text
工作区输入 ──→ Cursor 会话 ──→ BYOK 会话上下文 ──→ 模型
                                               │ CallMcpTool
                                               ▼
                                      cursor-devtools 分流
                                        │              │
                                        ▼              ▼
                                 Cursor 确认          回环浏览器桥
                                        │              │
                              桌面 local_run          Chrome 扩展
                                        │              │
                                        ▼              ▼
                                       Cursor 原生 MCP 卡片
```

```text
server/
├── prompt/cursor/devtools.json                   固定的 MCP 工具目录
├── prompt/cursor/devtools.md                     告诉模型何时用桌面工具、端口如何转发
├── src/cursor/tools/devtools.rs                   注入当前会话的 MCP 元信息
├── src/cursor/tools/tool_call_dispatch/devtools.rs 本机审批、执行与工具结果
├── src/cursor/tools/browser_bridge.rs             正在使用的浏览器、请求队列及结果关联
├── src/control/browser_devtools.rs                本机管理 API 中的浏览器桥
└── src/local_app/browser_host.rs                  通过 Chrome 原生消息告诉扩展 BYOK 在哪里
apps/desktop/src/features/
├── settings/BrowserDevToolsCard.tsx               显示浏览器是否已连接
└── calls/                                        调用筛选、上下文和模型请求统计
```

## 使用

1. 打开一次 Cursor BYOK，它会向 Chrome、Edge、Brave、Arc 和 Chromium 登记自己。
2. 在 Chrome 的 `chrome://extensions/` 里加载配套扩展。扩展会自动连接，不用配对；「系统设置 → 浏览器 DevTools」显示是否已连接。
3. 在 Cursor 里直接让它打开网站，或让它用你已经打开的网页。它会自己把用到的网页放进 Chrome 的蓝色「Cursor」标签组，不用手动拖。拖出标签组、在扩展弹窗里移出，或在调试提示条点「取消」，这个网页就收回了，它不能自己再拿走。登录状态就是 Chrome 里的登录状态。SSH 项目可以先用 Cursor 转发应用端口，再在桌面打开转发地址。
4. 在 Cursor 里使用浏览器工具。`local_run` 在执行前显示桌面目录和完整命令，选择「Run once」才执行。审批是一张独立的问题卡，回答后立即结束，命令结果显示在原来的 MCP 卡里。

工具目录随会话上下文追加。选页和录制状态留在扩展里。

```text
BYOK 启动 ──→ ~/.cursor-byok-v3/browser.json（地址和本次的令牌，仅本用户可读）
                        ▲ 读
Chrome ──→ 启动 BYOK 程序作为原生消息主机（只允许扩展的固定 ID）
                        │ 地址和令牌
扩展 ──→ /browser-devtools/enter、next、reply（凭令牌）
```

令牌每次启动都换，扩展会自己重新取。同一时间只用一个浏览器：先连上的在用，其他浏览器在弹窗点「改用这个浏览器」接手；在用的退出 60 秒后也会让出来。现有 `Shell`、Grep、文件读取和语言服务仍执行在当前工作区。

## 统计口径

调用页加载最近 500 条记录，可按模型、代理、会话和失败结果筛选。图上每根柱对应一条有输入用量的请求。API 回传公开汇总所需的用量、延迟、运行类型和工具轮计数。现有详情页仍负责按设置查看请求、响应与 Cursor 追踪。

- 有缓存读取的请求按报告缓存字段的请求计算。空字段保留为未知。
- 缓存输入占比按缓存读取量除以含缓存读写的输入总量计算。Anthropic 的普通输入与读写分别累计。支持上报实际上下文输入的供应商优先用该字段。
- 工具定义数按请求携带的工具 schema 记录。已发起工具调用按关联的已落库工具轮计算。缺少运行关联的历史调用显示未知。
- 首字与总耗时显示有实测值的请求中位数。输出速度使用从首字到完成的时间估算。

在一份含 4,784 条历史调用的真实 SQLite 副本上，最近 500 条查询的中位数约为 1.07 ms（原查询）和 1.09 ms（带运行关联的新查询）。两次只读测量的记录数和 31,121,408 字节文件大小均一致。该副本没有保留关联运行及工具轮，旧记录的代理和工具次数字段返回未知。
