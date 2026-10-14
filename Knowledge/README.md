# 审计与性能记录

以下文档记录对应日期的代码与测量。旧模块名称、行数、测试数量和设计建议属于历史上下文；
当前实现以 [架构指南](../ARCHITECTURE.md)、[库指南](../lib/README.md) 和
[当前验证状态](../PERFORMANCE.md) 为准。

| 日期 | 文档 | 范围与后续状态 |
| --- | --- | --- |
| 2026-08-15 | [残余渲染与托盘停顿静态审计](2026-08-15-static-audit-residual-stalls.md) | 针对文中列出的提交；没有独立运行图形测试 |
| 2026-10-10 | [代码流程、结构与调度深度分析](2026-10-10-code-flow-and-architecture-audit.md) | 未提交工作区快照；后续已修复精确锁状态、代次就绪、帧预算与事件唤醒等问题 |
| 2026-10-11 | [四轮性能测试](2026-10-11-performance-results.md) | CPU/GPU 组件测量、事件递送和已有接收端只读采样；无真实呈现 FPS 或端到端点击延迟结论 |

2026-10-15 的源码更新将项目拆为 `lib/`、`bin/`，保持任务栏隐藏修复，补充 SDK 公开 API、
可运行示例、文档示例与本地打包验证。57 项测试和 3 个文档示例通过；没有重跑四轮性能采样，
不能把历史数据写成本次拆分带来的速度收益。

## 原始性能数据

[performance/2026-10-11/](performance/2026-10-11/) 包含四轮结果及现场采样：

- [环境与版本](performance/2026-10-11/environment.json)
- [四轮汇总](performance/2026-10-11/aggregate.csv)
- [第一轮](performance/2026-10-11/run-1/results.json)、[第二轮](performance/2026-10-11/run-2/results.json)、[第三轮](performance/2026-10-11/run-3/results.json)、[第四轮](performance/2026-10-11/run-4/results.json)
- [进程采样](performance/2026-10-11/live-process.json)、[线程采样](performance/2026-10-11/live-threads.json)、[窗口线程对应](performance/2026-10-11/live-window-threads.json)

原始 JSON/CSV 保持测量时内容。复现入口已迁至
[bin/benches/support/measurements.rs](../bin/benches/support/measurements.rs)，当前命令见性能报告。
