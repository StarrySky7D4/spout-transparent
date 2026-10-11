# 性能测试结果（2026-10-11）

测试对象为当前事件＋独立期限架构。CPU 为 Intel Core Ultra 5 225H，14 核/14 线程；
GPU 为 Intel Arc 130T，驱动 32.0.101.6874。测试程序使用 Release 优化编译。
当前源码含尚未提交的改动，Git 基础版本为 a91550f106b69eb70320decf91b27f6426dd138e。

测试期间存在正在使用的 `VTubeStudioSpout` 来源，1280×720，格式代码 28。
已有接收端 PID 为 39968；本轮没有重启它、改变帧率/交互/裁切设置，也没有注册新的 Spout Sender。
其现场读数与独立的合成纹理组件测试分别记录。

## 1. 现场接收端读数

后续 8.01 秒只读采样在合成测试和编译完成后进行；应用的来源、动作与设置没有人为固定。

| 指标 | 读数 |
| --- | ---: |
| 整机口径 CPU 占用，按 14 个逻辑处理器归一化 | 0.7105% |
| 单个核心时间预算口径 CPU 占用 | 9.9469% |
| CPU 累计时间增量 | 约 0.797 秒 / 8.011 秒 |
| 工作集 | 23.41 MiB |
| 私有提交内存 | 51.23 MiB |
| GPU 3D 引擎利用率，末尾快照 | 9% |
| GPU 共享内存，末尾快照 | 28.93 MiB |

另一次 12.50 秒现场采样得到 CPU 0.6160%，GPU 3D 引擎 8 个采样点平均 10.375%，范围 9%～12%。
这次 CPU 采样与 Release Clippy 检查部分重叠，因此不视为受控空闲基线。
更早的一次 8.02 秒 CPU 读取为 0.8212%。现场数据会随头像动画、输入、驱动频率和后台负载变化。
工作集、私有提交和 GPU 共享内存含不同口径及共享资源，不能直接相加作为程序总内存。

窗口线程归属查询确认：主窗口线程 TID 10216 在 8 秒内使用 359.375 ms CPU，
约占一个核心预算的 4.486%；托盘窗口线程 TID 8024 的 CPU 增量在该采样精度下为零。
另外两个未命名线程分别使用 218.75 ms 和 171.875 ms；没有调用栈证据，不能直接将其归为驱动或某个模块。
这不是无 Sender、隐藏窗口或长期稳定性测试，也不能据此判断内存泄漏。

## 2. 合成纹理组件测试

以下单位均为毫秒。每轮预热后取样，共进行四轮；“中位值”是四轮各自中位数的中位数，
P95 列取四轮 P95 的最大值，便于展示观察到的尾部波动。前三轮使用 Release 测试入口，
第四轮使用最终独立 bench 入口运行同一测量代码；这样常规测试与性能测量不会争用 winit 的进程级事件循环单例。

| 测试项 | 1080p 中位值 | 1080p P95 | 4K 中位值 | 4K P95 |
| --- | ---: | ---: | ---: | ---: |
| 全屏 Draw，GPU 时间 | 0.1094 | 0.1699 | 0.2953 | 0.3989 |
| Clear＋全屏 Draw，GPU 时间 | 0.1276 | 0.1883 | 0.3236 | 0.4392 |
| R8 Alpha 提取＋staging Copy，GPU 时间 | 0.2415 | 0.3199 | 0.4995 | 0.9312 |
| staging 已可读后的 Map＋CPU 行复制 | 0.1444 | 0.3357 | 0.5633 | 0.9469 |
| CPU 可见边界扫描，中心小矩形 | 0.3678 | 0.5924 | 1.7635 | 2.3752 |
| CPU 可见边界扫描，全透明 | 0.3478 | 0.6388 | 1.6447 | 2.8310 |
| CPU 可见边界扫描，全不透明 | 0.0013 | 0.0015 | 0.0040 | 0.0062 |

中心矩形的宽和高各为源图的 1/4，面积占 1/16；其他位置 Alpha 为零。
全不透明时每行首尾迅速命中，因此扫描远快于含大量透明字节的图案。
GPU 测试使用静态、预先上传的普通 BGRA 纹理；没有将其模拟为真实来源的连续上传、共享纹理同步或窗口显示。

四轮 Draw 中位值范围：1080p 为 0.1053～0.1443 ms，4K 为 0.2507～0.3092 ms。
Clear 与不 Clear 在每轮中交替顺序测试，减少固定先后顺序导致的时钟与负载偏差。
本机这组 A/B 数据中，省去 Clear 的绘制区域 GPU 时间约减少 12%～16%（1080p）和 8%～10%（4K）；
这是同一绘制步骤的比较，不是架构改造前后的整体 FPS 提升。

GPU 时间由 Timestamp/Disjoint 查询测量，Disjoint 为真或频率无效的样本不纳入统计。
CPU 提交 Draw 的汇总中位耗时约 0.0037 ms（1080p）/0.0043 ms（4K），明显不同于 GPU 执行时间。
方法依据微软 [D3D11_QUERY 文档](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_query)
和 [时间戳有效性文档](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ns-d3d11-d3d11_query_data_timestamp_disjoint)。

测试器显式 Flush 并等待查询完成，测量开销不计入 Draw 的 GPU 时间；这种串行测试会改变 GPU 负载与频率。
GPU 时间戳完成不一定意味着非阻塞 Map 在 CPU 侧已经可用：生产代码的重试路径得到验证。
额外可读等待通常接近计时开销，观察到的最大值约 1.68 ms（1080p）/1.55 ms（4K），
其中含测试器 1 ms 重试睡眠的量化影响。Map＋复制一栏只包含最终成功的调用，不能当作整条回读链路的延迟。

## 3. 事件与调度

独立的真实 winit 事件循环接收 600 个有序事件，前 100 个作为预热，统计后 500 个；共四轮。
没有创建可见窗口或注册鼠标钩子；已有接收端的运行状态保持不变。

| 指标 | 结果 |
| --- | ---: |
| 跨线程 EventLoopProxy 递送中位值 | 0.0595 ms |
| 四轮中位值范围 | 0.0486～0.0626 ms |
| 四轮 P95 范围 | 0.2818～1.3429 ms |
| 全部样本中的最大值 | 5.7975 ms |
| 顺序和数量检查 | 每轮均完整收到 600 个事件，顺序正确 |
| 2 秒空等待尾段 AboutToWait 次数 | 每轮 2 次，包含进入等待及退出边界 |

该空等待小测没有包含接收端的发现线程、全局鼠标钩子或来源元数据监控，
不代表整个程序只有两次线程唤醒。递送耗时也不包含实际按钮处理、目标窗口转发或 Present 阻塞。

固定五槽 Scheduler 的纯内存操作微基准中位值约为 3.3 ns/循环；
该数值使用每组 1000 次操作摊销、含 black_box 屏障，只说明这个小数据结构的成本很低，
固定操作模式还可能被编译器进一步简化，不能代表完整 AboutToWait 或事件回调的成本，也不作为主要性能结论。

## 4. 对后续优化的判断

在合成 4K 测试中，CPU 掩码复制与边界扫描的中位耗时合计约 2.33 ms/样本，
明显大于单次 GPU 全屏 Draw 的 0.30 ms。但二者运行频率不同：前者通常约 10 Hz，后者取决于来源、限帧和呈现。
按 10 Hz 折算，该 CPU 部分约占一个核心预算的 2.33%；这是组件预算，不是现场进程占用的分解。

下一步优先验证：非交互模式使用 GPU 归约可见边界，只回读少量整数；
交互模式继续保留完整 Alpha 或评估分块/局部缓存。提高全图回读频率会直接放大复制与扫描成本。
现场主线程仍有可测 CPU 工作，Present 仍在 UI 线程；是否需要 RenderWorker 应通过进一步的 Present/显示时序采样决定。

本轮未采集 ETW/PresentMon，未得到接收端真实显示 FPS、Present 调用耗时分布、丢帧率或端到端点击延迟。
未重启或改动已有接收端，因此没有进行裁切开关、交互开关与 30/60/120 FPS 的受控 A/B。
现有结果足以比较本机组件成本，不能证明整体架构在所有场景下提升了某个性能百分比。

## 5. 复现与原始数据

可复用测试代码：[measurements.rs](/C:/Users/19825/Desktop/CodexProject/spout-transparent/benches/support/measurements.rs)。
独立 Cargo bench 可执行文件编译该模块，正常应用和常规测试不包含测量逻辑。

```powershell
$env:SPOUT_PERF_OUTPUT = 'target/performance/retest'
cargo bench --bench performance --locked --target-dir target/performance-build
```

独立构建目录避免 Windows 锁定正在运行的 `target/release/spout-transparent.exe`，测试无需停止已有接收端。

数据包含每个样本、各轮中位数/P95、现场进程与线程读取、硬件和运行二进制哈希：

- [四轮汇总 CSV](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/aggregate.csv)
- [第一轮原始结果](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/run-1/results.json)
- [第二轮原始结果](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/run-2/results.json)
- [第三轮原始结果](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/run-3/results.json)
- [第四轮 bench 验证结果](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/run-4/results.json)
- [现场线程采样](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/live-threads.json)
- [现场多点采样](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/live-process.json)
- [环境与版本信息](/C:/Users/19825/Desktop/CodexProject/spout-transparent/Knowledge/performance/2026-10-11/environment.json)
