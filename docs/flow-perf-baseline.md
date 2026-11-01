# Flow 性能基线

数字来自 GitHub Actions 上的 release API、PostgreSQL 16 和已登录的 Flow 页面。本文件由基准脚本生成，不是估计。

接口表里没写单位的数字都是毫秒。Lighthouse 用的是它默认的模拟限速，和 Playwright 那次不限速的导航不是同一种测量。

- 提交：`ccd0b9e9bb32875b4982af02d61a6e4d94806536`
- 分支：`refs/heads/cursor/flow-perf-bench-f8db`
- 运行：https://github.com/openprx/sylvode/actions/runs/41207820499
- PostgreSQL：PostgreSQL 16.15 (Debian 16.15-1.pgdg13+2) on x86_64-pc-linux-gnu, compiled by gcc (Debian 14.2.0-19) 14.2.0, 64-bit

## 接口

时间是客户端从发出请求到读完响应，单位毫秒。`semantic_patch` 每批 50 个块（`create_node` + `insert_text`，100 个操作，等于单次上限）。1k 是 20 批，5k 是 100 批。

| 项目 | 结果 |
| --- | --- |
| 注册 | 292.0 ms |
| 登录 | n=5 p50=269.4 p95=270.0 min=269.3 max=270.0 |
| 创建工作区 | 6.7 ms |
| 打开 Flow 开关 | 9.3 ms |
| 创建空页面 | n=5 p50=8.9 p95=10.8 min=6.0 max=11.2 |
| semantic_patch 1k 每批 | n=20 p50=122.8 p95=231.2 min=26.3 max=237.0 |
| semantic_patch 1k 总墙钟 | 2656.2 ms，完成 20/20 批 |
| semantic_patch 5k 每批 | n=100 p50=694.7 p95=1360.2 min=20.1 max=1516.9 |
| semantic_patch 5k 总墙钟 | 71773.4 ms，完成 100/100 批 |
| 5k 文档上再打一批 | 364.3 ms |
| bootstrap 1k | n=5 p50=7.6 p95=9.5 min=7.2 max=9.6 |
| bootstrap 5k | n=5 p50=21.2 p95=21.3 min=20.6 max=24.3 |
| bootstrap 5k 响应字节 | n=5 p50=962214.0 p95=962214.0 min=962214.0 max=962214.0 |
| 搜索索引追上 5k 页 | 2037.0 ms |
| 搜索 flowbench5k | n=5 p50=26.9 p95=27.0 min=26.9 max=27.6 |
| 搜索 benchblock | n=5 p50=98.3 p95=99.0 min=97.9 max=99.0 |
| 发表评论 | n=20 p50=3.1 p95=3.8 min=3.0 max=4.6 |
| 评论列表 | n=5 p50=3.0 p95=3.1 min=2.6 max=3.1，条数 20 |
| 移动小页面 | 24.4 ms |
| 移动 1k 页面 | 31.5 ms |

5k bootstrap 最后一次：snapshot_seq=0，head_seq=101，tail=101，snapshot base64 字符=404，响应 962214 字节。

## 两个 API 进程的扇出

进程 A 接受 REST `insert_block`。进程 B 只通过 LISTEN/NOTIFY 和 50ms 轮询把已提交更新送到自己的 WebSocket。端到端从发出 REST 前一刻算到 B 收到 `accepted`。响应后延迟是 HTTP 响应已经返回之后还要再等的时间，可能为负（帧比响应先到）。第一笔是预热，不计入。

- 状态：完成
- 端到端：n=8 p50=12.2 p95=12.5 min=11.5 max=12.6
- HTTP：n=8 p50=11.2 p95=11.7 min=10.9 max=11.7
- 响应返回之后：n=8 p50=0.2 p95=0.6 min=0.1 max=0.7

## Loro 加载、apply、快照

同一台 runner 上的 release 二进制，进程内调用生产适配器，不经过 HTTP。

### 1000 块

- apply 2000 个操作：n=3 p50=9.6 p95=9.6 min=9.5 max=11.9
- export snapshot：n=5 p50=0.2 p95=0.2 min=0.1 max=0.2，快照 96116 字节
- load snapshot：n=5 p50=3.5 p95=3.8 min=3.4 max=4.4
- 在已加载文档上 import 一条更新（214 字节）：n=5 p50=0.2 p95=0.2 min=0.2 max=0.3

### 5000 块

- apply 10000 个操作：n=3 p50=52.4 p95=52.4 min=50.2 max=55.7
- export snapshot：n=5 p50=1.1 p95=1.1 min=0.8 max=1.1，快照 495413 字节
- load snapshot：n=5 p50=20.6 p95=21.2 min=20.2 max=24.1
- 在已加载文档上 import 一条更新（249 字节）：n=5 p50=0.8 p95=0.8 min=0.8 max=1.0

## EXPLAIN (ANALYZE)

语句是这些路径上的 SQL，不是把 Rust 鉴权循环也算进去。执行时间单位毫秒，含缓冲区数字。

| 查询 | 计划 | 执行 | 根节点 | 行数 | shared hit | shared read |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| bootstrap_document_5k | 1.0 | 0.1 | Nested Loop | 1 | 9 | 0 |
| bootstrap_tail_5k | 0.6 | 0.3 | Sort | 101 | 23 | 0 |
| comments_list_1k_page | 0.5 | 0.1 | Limit | 20 | 11 | 0 |
| fanout_poll | 0.4 | 0.2 | Limit | 130 | 7 | 0 |
| object_view_5k | 0.8 | 0.1 | Nested Loop | 1 | 13 | 0 |
| search_benchblock | 1.9 | 93.8 | Limit | 2 | 137 | 0 |

## 前端

Playwright 打开已登录、Flow 开关已打开的对象页。Lighthouse 连的是同一套登录态；连不上时下面会写原因，Web Vitals 仍然单独记录。

### 1k

- 最终 URL：`http://127.0.0.1:18080/workspace/6920d0a1-1b2a-41b6-a4bd-51afecf68f20/flow/6cd98218-23bd-4b16-ba92-dc4ab99420c4`
- 导航 TTFB 6.5 ms，FCP 256.0 ms，LCP 348.0 ms，DCL 61.6 ms，CLS 0.016283191805448387
- 画布出现（脚本墙钟）561.0 ms，编辑器可输入 597.0 ms
- 截图：screenshots/flow-page-1k.png
- 打字：n=20 p50=7.9 p95=15.4 min=5.8 max=20.6

### 5k

- 最终 URL：`http://127.0.0.1:18080/workspace/6920d0a1-1b2a-41b6-a4bd-51afecf68f20/flow/2a7ef472-2cf8-4f3b-b329-8f043f039b22`
- 导航 TTFB 2.3 ms，FCP 132.0 ms，LCP 168.0 ms，DCL 35.2 ms，CLS 0.016283191805448387
- 画布出现（脚本墙钟）663.0 ms，编辑器可输入 702.0 ms
- 截图：screenshots/flow-page-5k.png
- 打字：n=20 p50=23.7 p95=29.1 min=21.8 max=33.0

Lighthouse 性能分 0.26，LCP 27398.8 ms，CLS 0.016236，FCP 4383.9 ms，TBT 1339.5 ms。

## 怎么读

这是一次 Actions runner 上的基线，不是容量上限，也没有和 Notion 对比。扇出延迟包含最多约 50ms 的轮询兜底。侧栏仍然只拉 100 个对象，这次没有把那个上限算进延迟。
