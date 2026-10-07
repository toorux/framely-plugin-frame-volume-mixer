# 2026-10-07 验证记录

设备：Steam Frame，SteamOS，aarch64，steamos UID 1000，PipeWire 1.6.8。

| 项目 | 结果 |
| --- | --- |
| Rust unit tests | 3 passed |
| TypeScript | tsc --noEmit passed |
| ARM64 release | aarch64-unknown-linux-gnu 构建成功 |
| Framely 包验证 | ID tooru.volume-mixer / version 0.1.0 |
| 原生与 PulseAudio 流 | pw-play 和 paplay 均发现，PID 与子进程对应 |
| 原生流音量 | 设置 35%，读回 35% |
| 音量隔离 | 原生流调节不改变 PulseAudio 流 |
| 静音隔离 | PulseAudio 流静音不改变原生流 |
| 取消静音 | 成功 |
| 对象保护 | 过期 serial、已退出流、系统链路写入均拒绝 |
| 生命周期 | start/stop 与 EOF 正常退出 |
| 安装后 RPC | connected=true，读取到 Waydroid ALSA Playback |
| 安装后服务 | framely-backend-tooru.volume-mixer.service active |
| 模拟 UI | 键盘 75%→74% 读回，多流展开成功 |

安装包 SHA256：`d4868e0f8dfb0b952e6f4a3ef73df7199e9368daec62fe945c1cb08826bdb22a`。

未验证：非零音频电平检测（尚未实现），每个 Android 应用内部混音控制（宿主仅暴露汇总流），佩戴设备的 VR 指针操作。

## 0.2.0 更新验证

- 后端 4 项 Rust 测试、TypeScript 检查和 ARM64 release 构建通过。
- 新增默认输出设备发现，系统总音量读回与 `wpctl get-volume @DEFAULT_AUDIO_SINK@` 一致（41%）；写回原值成功，不改变测试前用户音量。过期输出序列号请求被拒绝。
- `tests/device.py` 原有应用流隔离、静音、生命周期和保护检查继续通过。
- 模拟 UI：系统音量键盘 41% 调到 40%，480px 快捷页每应用一行，无横向溢出。
- 新包 SHA256：`fda1828bb4e37b5bdf39d0c1e6593fde33daf9db95d0a9ac363a8f71ef643afd`。

## 0.2.1 紧凑布局

仅修改 UI 和清单版本，后端保持原有行为。TypeScript、ARM64 载荷构建及 Framely 包校验通过；模拟页键盘 75%→74%、多流展开/收起、窄屏与 1200×800 布局正常。

## 0.2.2 平面分区与胶囊滑块

仅修改样式与清单版本。类型检查、ARM64 载荷构建、Framely 包验证通过；浏览器确认卡片已移除，胶囊轨道正常，模拟系统音量键盘 41%→40% 正常读回。

## 0.2.5 验证

- Rust 5 项测试及 TypeScript 类型检查通过。
- 设备上通过音频主机名识别哔哩哔哩HD、明日方舟：终末地，并成功读取两者 PNG 图标（分别约 21KB / 90KB 的数据 URL）。
- ARM64 临时后端通过原生/Pulse 独立调音量、静音、旧序号和退出流拒绝等设备回归；仅对测试静音流改值，系统总音量写回原值。
- 外部修改测试流音量后，状态事件在 760ms 内读到新值。
- 浏览器模拟返回旧值、延迟 500ms 再更新状态：系统单步调音量和多流应用调音量均连续采样 75 次，无回跳。
- 浏览器确认应用图标已加载且尺寸 40px；百分比与滑块中心重合，鼠标命中测试返回 INPUT，文字 pointer-events 为 none；系统区 border-bottom 为 0px。
- 界面截图：docs/preview-v025.png（真实设备状态与图标的模拟预览）。

## 0.2.6 验证

- 类型检查、5 项 Rust 测试通过；新增覆盖非默认输出设备控制、失效设备序号拒绝。
- 设备回归通过输出列表枚举、输出设备写回原音量、默认设备写回当前选择，以及失效输出音量控制/默认切换拒绝。
- 浏览器模拟切换至 Echo-Cancel Sink 后，顶部系统音量跟随变为 100%；单独调整该设备到 99% 后，顶部同步显示 99%，其他输出保持原值。
- 全部 Audio/Sink 设备显示在列表中，包含虚拟输出标记；截图 docs/preview-v026.png。
- 0.2.6 安装包通过 framely verify。

## 0.2.8 验证

- TypeScript 类型检查及 test:i18n 通过，覆盖 zh-CN/zh-TW/zh-Hans 等中文识别、其他语言回退英文、数量插值、保留应用原名、后端错误翻译及 UI 文案字典覆盖。
- 接入宿主 language.get（读取已解析的 language 字段），订阅 language.changed；不使用 preference=auto 自行猜测语言，不提供语言选择控件。
- 浏览器验证中文显示中文，fr-FR 显示英文；宿主语言事件由 zh-CN 改为 fr-FR 时，当前页面立即更新为英文，音量值保持不变。
- 界面截图 docs/preview-v028-zh.png 和 docs/preview-v028-en.png。
- 0.2.8 安装包通过 framely verify。

## 0.2.9 验证

- TypeScript 类型检查和 i18n 测试通过，安装包通过 framely verify。
- 在模拟 Framely 宿主背景的 iframe 中确认插件 body 为 rgba(0,0,0,0)，输出设备区域不存在，应用图标和空白占位边框为 0px，容器内边距为 16px 28px。
- 预览截图 docs/preview-v029.png；截图深色底来自预览宿主，不来自插件。
