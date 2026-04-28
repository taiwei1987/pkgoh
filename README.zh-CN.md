# pkgoh

简体中文 | [English](README.md)

`pkgoh` 是一个开源的 macOS 终端资产管理器，面向开发者常用的软件包生态。

安装完成后，用户只需要输入 `pkgoh`，或者使用简写 `pkg`，就能进入一个键盘优先的 TUI 界面，统一扫描、筛选、查看、删除和清理多种包管理器安装的全局工具。

## 界面预览

下面是更接近当前版本结构的示意布局：

```text
┌ PKGOH · Asset Console ───────────────────────────────────────────────┐
│ Total 54  Selected 1  Reclaim 192.4MB  Sort Size↓   Search hidden    │
├────────────────────────────────────┬─────────────────────────────────┤
│ Asset List                         │ Details                         │
│ 1. ○ claude-code   Homebrew 192MB  │ claude-code  Homebrew           │
│ 2. ● @google/gemini npm     173MB  │ Summary & Advice                │
│ 3. ○ python@3.11  uv        63MB   │ ...                             │
│ ...                                │ ...                             │
├────────────────────────────────────┴─────────────────────────────────┤
│ Action Console                                                       │
│ Delete Confirmation                                                  │
│ Remove 1 selected item · Reclaim 192.4MB                             │
│ Press Delete again or Enter to run · Esc to cancel                   │
├──────────────────────────────────────────────────────────────────────┤
│ [↑↓] Move  [0-9] Jump  [/] Search  [Space] Select  [Delete] Remove   │
│ [C] Clean Cache  [R] Refresh  [S] Sort  [Esc] Back/Quit              │
└──────────────────────────────────────────────────────────────────────┘
```

## 为什么做 pkgoh

开发者在 macOS 上安装工具时，往往会同时使用 Homebrew、npm、pnpm、cargo、pip、uv、mas 等多个来源。时间久了以后，很难快速回答这些问题：

- 现在到底装了哪些全局工具
- 哪些工具最占空间
- 哪些工具已经很久没用过了
- 哪些工具大概率可以删
- 真正执行删除前，能释放多少空间

`pkgoh` 的目标，就是把这些信息统一收敛到一个终端界面里。

## 当前能力

- 扫描 Homebrew、npm、pnpm、cargo、pip、uv、mas
- 用统一列表展示名称、来源、版本、空间大小、最后使用时间
- 启动时显示加载态，扫描过程中持续反馈，不让界面看起来像卡死
- 支持按空间大小排序
- 对大体积项目和长期未使用项目做高亮
- 支持多选，并实时显示预计可释放空间
- 支持用 `/` 进入搜索过滤，按名称快速定位
- 支持数字跳转、刷新确认、退出确认
- 支持真正可执行的删除和缓存清理操作
- 右侧常驻详情栏，直接显示详细信息，不再需要单独的详情页
- 当系统语言是简体中文时，界面自动尽量切换为中文文案

## 当前支持的来源

内置扫描器目前支持：

- Homebrew formula
- Homebrew cask
- npm 全局包
- pnpm 全局包
- cargo 安装的二进制
- pip 全局包
- uv 管理的 Python 运行时
- uv tool 安装的工具
- 通过 `mas` 管理的 Mac App Store 应用

扫描架构本身是插件式的，后续可以继续扩展到更多管理器。

## 交互方式

- `↑` / `↓`：上下移动
- `Space`：选中或取消选中当前项
- `Delete`：准备删除已选项目
- `C`：准备清理已选项目缓存
- `R`：准备刷新列表
- `/`：进入实时搜索过滤
- `S`：按空间大小从大到小排序
- `Esc`：退出搜索、取消待确认操作，或进入退出确认
- `Enter`：确认当前待执行操作
- `0-9`：按数字跳转

## 评估建议分级

每个项目都会被归入三类建议之一：

- `可删除`：通常删除风险较低，主要影响当前工具本身
- `建议保留`：删除后可能会有一些后续影响，但通常还能处理
- `核心依赖`：很可能影响其他工具或工作流，对于不懂开发的用户恢复成本较高

右侧详情栏会同时给出评估说明。

## 删除与权限说明

`pkgoh` 执行的是真实删除，不是模拟删除。

需要注意：

- 删除和清缓存都必须先选中，再确认执行
- 有些操作不需要管理员权限，会直接执行
- 有些操作需要管理员权限，尤其是部分 Homebrew cask 或系统级删除
- 当确实需要管理员权限时，`pkgoh` 会尽量把密码输入和错误反馈留在 TUI 的操作区里，而不是让隐藏的终端提示破坏界面体验
- 删除成功后，当前列表会直接更新，不会每次都强制整表重扫

## 安装方式

### 方式一：从源码安装

请先安装 Rust，然后执行：

```bash
cargo install --path . --root ~/.local
```

如果你的 `~/.local/bin` 已经在 `PATH` 中，那么可以直接输入：

```bash
pkg
```

或者：

```bash
pkgoh
```

### 方式二：下载 GitHub Release 二进制包

仓库自带 GitHub Actions 发布脚本，会自动构建：

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

每个发布压缩包中会包含：

- `pkg`
- `pkgoh`
- `README.md`
- `README.zh-CN.md`
- `LICENSE`
- `pkgoh.example.toml`

解压后，把 `pkg` 和 `pkgoh` 放到你已经加入 `PATH` 的目录即可。

## 配置文件

默认配置路径：

```text
~/.config/pkgoh/pkgoh.toml
```

也可以通过环境变量指定自定义配置：

```bash
PKGOH_CONFIG=/path/to/pkgoh.toml pkg
```

示例配置：

```toml
[sources]
brew = true
npm = true
pnpm = true
cargo = true
pip = true
uv = true
mas = true

[highlight]
large_size_mb = 500
unused_days = 90
```

## 项目结构

代码当前主要分为这些层：

- `src/model.rs`：统一资产模型与展示辅助方法
- `src/plugins.rs`：各包管理器扫描器与扫描总线
- `src/actions.rs`：删除与清缓存执行层
- `src/app.rs`：TUI 布局、交互状态、确认流程、反馈信息
- `src/i18n.rs`：系统语言检测与中英文切换
- `src/config.rs`：配置读取与默认值

`pkg` 和 `pkgoh` 两个入口，最终都走同一套库逻辑。

## GitHub 自动发布

`.github/workflows/release.yml` 会在你推送类似 `v0.1.0` 这样的 tag 时，自动构建 Intel 和 Apple Silicon 两种 macOS 二进制，并上传发布压缩包。

## 已知限制

- 当前只能识别已经支持的这些包管理器
- 手动安装、未支持的管理器安装、或者自定义脚本安装的工具，可能不会显示出来
- 空间大小和最近使用时间属于基于文件路径与元数据的 best effort 估算，不一定 100% 精确
- 依赖关系判断属于启发式评估，适合作为建议，但不能当成绝对保证
- 某些系统级删除本身就会比较慢，因为底层包管理器还会做自己的卸载与清理工作

## 参与贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## License

MIT，见 [LICENSE](LICENSE)。
