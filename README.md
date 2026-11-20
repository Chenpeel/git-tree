# git-tree

使用 Rust 原生 GUI 和鼠标 TUI 查看本地 Git 提交, 分支和合并关系, 无需系统 Git.

- 默认打开 **Git 3D**, 使用球形 commit 节点和细彩色曲线, 不使用竹子, 树皮, 树叶或植物模型.
- 在窗口顶部切换 **History 2D**, 使用固定行距轨道和对齐的提交 ID, 标题/引用, 作者与时间.
- 启动只读取提交对象和引用, 不扫描所有提交的文件树, 不统计增删行或 LFS/二进制资源量.
- 仅在用户选中提交或显式点击读取时加载该提交的文件变更和 diff; 启动默认定位 HEAD 不触发内容读取.
- 支持节点/整行点击, 搜索, HEAD 定位, 路径高亮和仓库变化自动刷新.
- 使用内嵌 `gix` 直接读取本地仓库, 不查找或调用 `git`, `git-lfs`, 外部 diff 或 filter.
- 保持目标仓库只读, 不执行 checkout, commit, reset, fetch 或网络请求.

## 快速开始

在任意本地仓库运行:

```bash
git-tree
```

指定仓库, 默认 Git 3D:

```bash
git-tree /path/to/repo
```

直接打开二维轨道:

```bash
git-tree --view rails /path/to/repo
```

打开终端界面:

```bash
git-tree --tui /path/to/repo
```

默认 `--view 3d`. 窗口内点击 `Git 3D` / `History 2D` 切换并保留选择, GUI 视图参数与 `--tui` 不组合使用.

支持普通仓库, bare 仓库和 linked worktree. 运行预编译文件不需要 Git 或 Rust, GUI 仍需要系统图形环境和驱动.

## 从源码构建和安装

### 准备工具链

- 安装 Rust 1.88 或更新版本, Cargo 和平台 C/C++ 链接工具链.
- Linux: 准备 `pkg-config`, X11/Wayland 图形开发环境和 OpenGL 驱动. Debian/Ubuntu 常用包为 `build-essential`, `pkg-config`, `libx11-dev`, `libxi-dev`, `libxrandr-dev`, `libxcursor-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libgl1-mesa-dev`.
- macOS: 准备 Xcode Command Line Tools, 使用 Apple Silicon 或 Intel 对应的本机 Rust 工具链.
- 首次构建允许 Cargo 从 crates.io 下载依赖, 保留 `Cargo.lock` 并使用 `--locked`.

### 安装到个人目录

在本项目目录运行:

```bash
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" \
  cargo install --locked --path . --root "$HOME/.local"
```

将 `$HOME/.local/bin` 加入 `PATH`, 然后检查入口:

```bash
command -v git-tree
```

### 安装到自定义目录

```bash
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" \
  cargo install --locked --path . --root /path/to/prefix
```

将 `/path/to/prefix/bin` 加入 `PATH`. 不要将 Cargo 安装根目录设为 `/usr/bin`, 否则实际入口会成为 `/usr/bin/bin/git-tree`.

### 安装到 Linux 系统目录

先以普通用户构建, 仅复制最终文件时提权:

```bash
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo build --locked --release
pkexec install -m 0755 "$HOME/.cache/git-tree/target/release/git-tree" /usr/bin/git-tree
```

优先使用个人目录或 `/usr/local/bin`, 避免覆盖系统包管理文件. macOS 的 `/usr/bin` 受系统保护, 使用 `$HOME/.local/bin` 或其他可写目录.

### 仅构建 TUI

```bash
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" \
  cargo install --locked --path . --no-default-features --root "$HOME/.local"
```

运行时添加 `--tui`. 此构建不链接 GUI 依赖, 不自动将 GUI 启动失败降级为 TUI.

## 参数

```text
git-tree [OPTIONS] [PATH]

[PATH]                       仓库路径或仓库内目录, 默认当前目录
--tui                        打开鼠标终端界面
--view <3d|rails>             GUI 视图, 默认 3d
--main-branch <NAME>          指定主干分支或完整引用名
--max-commits <COUNT>         限制加载数量, 必须大于 0
--licenses                   输出可视化资源声明, 不需要仓库
-h, --help                   显示帮助
-V, --version                显示版本
```

示例:

```bash
git-tree --view 3d --main-branch develop /path/to/repo
git-tree --view rails --main-branch refs/remotes/origin/main /path/to/repo
git-tree --tui --max-commits 5000 /path/to/repo
```

默认读取全部可达历史. 大型仓库可显式设置 `--max-commits`, 界面会显示 `HISTORY LIMITED`. 限制模式从主干, HEAD 和各引用向祖先广度遍历, 不代表按日期选取最近 N 条.

## 加载行为与性能边界

- 启动只读取 commit 的父关系, 作者, 时间, 说明和引用, 不比较所有提交的 tree/blob.
- 不提供全历史代码量, LFS/二进制量统计, 两种 GUI 及 TUI 均无统计列或变更条.
- 初始自动选择 HEAD 只显示元数据. 点击节点/行, 搜索结果, HEAD, 键盘选择或 `Load file changes` 后才读取该提交的变更.
- 后台刷新保留选中提交, 不自动重复请求其 diff. 当前详情已读取时继续保留.
- 提交图加载和空间/轨道布局仍与提交数量和边数相关, 不保证任意规模仓库瞬时打开.
- 文件 diff 在后台按第一父树比较, 递归跳过相同子树. 真正根提交使用空树, 缺失父提交不伪造根 diff.
- 单个 blob 预览上限为 512 KiB, 整体 diff 预览约 2 MiB 或 3 秒处理预算; 超出时提示, 保留变更文件列表. 该预算不覆盖文件列表的树遍历或单次磁盘读取.
- LFS pointer 仅在选中提交的预览中显示 oid/声明大小, 不下载实体或计算资源汇总. 二进制/非 UTF-8 文件显示说明而非文本 diff.

## Git 3D 操作

| 操作 | 方法 |
|---|---|
| 查看提交和按需 diff | 单击球形节点 |
| 聚焦节点 | 双击节点 |
| 旋转 | 左键拖动 |
| 平移 | 右键或中键拖动 |
| 缩放 | 鼠标滚轮 |
| 适配全图 | `Fit graph` 或 `F` |
| 恢复视角 | `Reset view` |
| 定位 HEAD | `HEAD` 或 `H` |
| 读取当前提交变更并定位 | Enter |
| 搜索 | 输入 ID 前缀, 标题, 作者或分支名 |
| 下一条结果 | `Next match` 或在搜索框按 Enter |
| 切换二维 | `History 2D` |
| 取消路径高亮 | `Show all paths`, 关闭 `Path` 或 Esc |
| 收起面板 | `Branches` / `Inspector` |
| 手动刷新 | `Refresh` 或 `R` |

- 将 main 第一父链放在中央, 其余分支向三维空间展开. 所有真实父边保持连接, 合并边较细.
- 节点位置由拓扑决定, 不代表日期间隔或代码量. 多根/浅克隆显示已知边界.
- 远处减少普通节点标记与常驻文字, 放大或搜索聚焦查看提交, 不删除历史数据.
- 默认隐藏详情侧栏, 点击节点后展开. 选中提交的元数据不需要读取文件内容.
- GUI 需要 OpenGL 3.3 和 24-bit 深度缓冲. 3D shader 在图形驱动中编译, Rust 构建不能证明驱动兼容性.

## History 2D 操作

- 点击 `History 2D` 或通过 `--view rails` 打开.
- 固定 44 点行高, 左侧显示紧凑轨道, 右侧对齐提交 ID, 标题/当前引用, 作者和时间.
- 使用可复用通道与长连接旁路, 轨道不代表永久分支归属.
- 单击整行读取提交文件变更, Up/Down 选择, `Locate` 定位当前行.
- 使用滚轮滚动, 列宽不足时横向滚动. 仅绘制可见行, 固定表头.
- 刷新按顶部可见提交 ID 保留滚动锚点, 不因新增提交自动回到顶部.
- 选择路径只高亮可达祖先, 淡化其余连接, 不过滤实际提交或合并关系.

输入搜索内容时不触发单字母快捷键. 搜索侧栏最多展示前 100 项, `Next match` 遍历全部结果. 右侧 diff 可独立滚动. 中文内容优先使用常见系统 CJK 字体, 无对应字体时安装系统字体.

## TUI 操作

使用支持鼠标上报的终端. 部分终端按住 Shift 可选择复制文本.

| 按键或鼠标 | 功能 |
|---|---|
| 单击提交行 | 读取该提交变更 |
| Enter | 读取当前提交变更; 搜索模式中定位下一条结果 |
| `j` / Down, `k` / Up | 选择下一个/上一个提交 |
| Home / End | 选择最新端/最早端提交 |
| 图形区域滚轮 | 滚动历史 |
| Left / Right | 横向平移 |
| 详情区域滚轮 | 滚动详情 |
| PgUp / PgDn | 按页滚动详情 |
| `/` | 编辑搜索条件 |
| `n` | 定位下一条结果 |
| `h` | 定位 HEAD |
| `r` | 刷新 |
| `q`, Esc, Ctrl+C | 退出 |

图例: `●` 普通提交, `◆` 合并提交, `◌` 未加载祖先边界. 粗线为主干, 细线为第一父连接, 虚线为其他父连接, 交叉不表示额外提交.

## Git 语义与限制

- 按拓扑顺序排列, 不强行按日期排序破坏父子方向.
- 主干优先显式参数, 然后本地 main, master, 当前分支和可用引用. 不修改分支名.
- 只显示分支/tag/本地已有远程跟踪引用与 HEAD 可达历史; 不读取 reflog-only/悬空提交, 不连接远程或扫描多个仓库.
- 分支名表示当前指向, 不能恢复 Git 未可靠保留的分支创建/历史 checkout 记录.
- 监听 Git 目录及 worktree 公共目录, 对事件去抖约 220 ms. 时间取决于仓库和文件系统.
- 监听失败时显示 `WATCH ERROR`, 手动刷新并检查资源或权限, 不静默掩盖失败.
- 对浅克隆标识边界. 重命名按删除和新增显示, 不展开子模块历史, 不执行 textconv 或 filter.
- 仅读仓库自身配置, 不读取系统 Git 安装配置, 用户 Git 配置, 外部 include 或 Git 环境覆盖.

## 构建与分发

使用 `.github/workflows/build.yml` 在 Linux x86_64, macOS Apple Silicon 与 macOS Intel runner 构建 GUI 和 TUI-only 版本. 推送 `v*` tag 后生成归档, SHA-256 和 Release; 实际发布前不假定已有下载文件.

预编译文件可复制到 PATH 中的目录, 仍依赖平台窗口库/OpenGL, 不承诺跨所有 Linux 发行版通用. macOS 未配置 Apple 签名和公证.

当前图形全部程序生成, 不包含第三方植物模型或贴图, 运行时不下载美术素材. 仓库和 Release 仅保留当前实现.

## 项目结构

```text
src/main.rs         入口与默认 3D / 二维 / TUI 选择
src/repository.rs   提交/引用读取, 拓扑排序, 按需文件变更和 diff
src/changes.rs      变化子树遍历, LFS/二进制说明和有界 blob 预览
src/graph.rs        共享节点与分支链
src/rails.rs        紧凑轨道, 合并旁路与祖先路径
src/worker.rs       监听和后台按需读取
src/state.rs        选择, 搜索, 元数据与显式变更读取状态
src/gui.rs          视图切换, 交互和侧栏
src/history.rs      无统计的二维轨道提交表
src/scene.rs        抽象三维布局, 相机与遮挡
src/renderer.rs     球形节点和彩色曲线的实例化绘制
src/tui.rs          无统计鼠标终端界面
```

执行静态检查与编译, 不把构建通过当作视觉, 性能或跨平台实测通过:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo check --locked
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo check --locked --no-default-features
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo clippy --locked -- -D warnings
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo clippy --locked --no-default-features -- -D warnings
CARGO_TARGET_DIR="$HOME/.cache/git-tree/target" cargo build --locked --release
```
