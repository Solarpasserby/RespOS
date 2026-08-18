# RespOS Rust 工具链与外部依赖管理

本文面向刚开始接触 Rust 工程管理的队员，解释 `rustc`、`rustup`、`cargo`、toolchain、target、component、
crate、feature、lockfile 和 vendor 的关系，并给出 RespOS 当前可复现的操作方式。

先记住三句话：

1. `rustup` 决定“调用哪一套 Rust 工具”；
2. `rustc` 负责编译一个 Rust crate，`cargo` 负责组织整个工程和依赖图；
3. “缓存里有源码”不等于“项目能离线构建”，必须用固定工具链、固定 lockfile、真实目标架构执行
   `--offline --locked` 验证。

## 1. 组件关系

```text
rustup
├── stable / beta / nightly-YYYY-MM-DD 工具链
│   ├── rustc              Rust 编译器
│   ├── cargo              包管理器和构建编排器
│   ├── rust-std(target)   某个 target 的 core/alloc/std 预编译库
│   ├── rust-src           Rust 标准库源码
│   ├── llvm-tools         objcopy/size 等底层工具所需组件
│   ├── rustfmt            格式化器
│   ├── clippy             静态检查
│   └── rust-analyzer      编辑器语言服务器
└── override 规则         为当前命令或目录选择工具链

Cargo
├── Cargo.toml             人写的包信息、依赖范围、feature、profile
├── Cargo.lock             Cargo 解析出的精确版本和 checksum
├── registry/git cache     用户机器上的下载缓存
├── vendor/                可复制、可审计的依赖源码快照
└── target/                可删除并重新生成的编译产物
```

`rustup target add` 主要安装目标的 Rust 标准库，不会自动安装外部 C 编译器、linker、QEMU、固件或系统库。
RespOS 还需要 Makefile、linker script、lwext4 CMake 和对应平台的镜像/启动参数共同完成构建。

## 2. rustc：真正的编译器

常用检查：

```bash
rustc --version
rustc -Vv
rustc --print host-tuple
rustc --print target-list | rg 'riscv64|loongarch64'
```

`rustc -Vv` 比短版本更适合保存证据，它包含 commit、日期、host 和 LLVM 版本。Rust 源文件中
`#![no_std]` 表示不依赖操作系统提供的标准库；但内核若有全局 allocator，仍可显式使用 `alloc` 中的
`Vec`、`String` 和集合。

通常不要直接手写长 `rustc` 命令构建 RespOS。Cargo 负责正确传递 crate graph、feature、target、profile
和 linker 参数，顶层 Makefile 再负责用户程序嵌入、四个平台配置和最终产物复制。

## 3. rustup：工具链版本管理器

### 3.1 channel 与日期 nightly

Rust 常见 channel：

- `stable`：稳定发布；
- `beta`：下一稳定版的预发布；
- `nightly`：每天构建，可使用未稳定能力；
- `nightly-2025-01-18`：固定到某一天，适合可复现工程。

RespOS 正式提交固定为：

```text
nightly-2025-01-18
rustc 1.86.0-nightly (6067b3631 2025-01-17)
```

仓库根目录 `rust-toolchain.toml` 声明该版本，顶层 `Makefile` 还会覆盖导出
`RUSTUP_TOOLCHAIN=nightly-2025-01-18`。这比“我本机 nightly 能编译”更严格，也更接近比赛平台。

### 3.2 安装 RespOS 所需工具链

```bash
rustup toolchain install nightly-2025-01-18 --profile minimal
rustup component add --toolchain nightly-2025-01-18 llvm-tools-preview
rustup target add --toolchain nightly-2025-01-18 \
  riscv64gc-unknown-none-elf loongarch64-unknown-none
```

检查：

```bash
rustup toolchain list
rustup target list --toolchain nightly-2025-01-18 --installed
rustup component list --toolchain nightly-2025-01-18 --installed
rustc +nightly-2025-01-18 -Vv
cargo +nightly-2025-01-18 -V
```

`minimal` profile 已包含 `rustc`、Cargo 和 host 标准库；其他 component 按需添加，不要安装几乎必然因
缺件失败且体积很大的 `complete` profile。

### 3.3 为什么当前目录可能没有使用 toolchain 文件

rustup 选择优先级中，命令行 `+toolchain` 和环境变量 `RUSTUP_TOOLCHAIN` 高于目录中的
`rust-toolchain.toml`。诊断时执行：

```bash
rustup show active-toolchain
env | rg '^RUSTUP|^RUSTC|^CARGO'
rustc -Vv
RUSTUP_TOOLCHAIN=nightly-2025-01-18 rustc -Vv
```

2026-08-18 当前开发容器的交互环境变量把普通 `rustc` 选择为 `nightly-2025-05-20`（Rust 1.89 nightly），
但顶层 Makefile 正式构建仍强制 2025-01-18（Rust 1.86 nightly）。因此编辑器没有报错，不代表课程平台
工具链可以编译；提交验证始终走顶层 Makefile。

## 4. target、host 和交叉编译

host 是运行编译器的机器，当前通常是：

```text
x86_64-unknown-linux-gnu
```

target 是产物运行的平台。RespOS 主要使用：

```text
riscv64gc-unknown-none-elf
loongarch64-unknown-none
```

三元组中的 `unknown-none` 表示没有宿主操作系统，不能默认使用文件、线程、socket 等 `std` API。依赖
必须支持 `no_std`，需要动态内存时还要确认它只要求 `alloc`，而不是偷偷启用 `std`。

最小检查方式：

```bash
cargo +nightly-2025-01-18 check --target riscv64gc-unknown-none-elf
cargo +nightly-2025-01-18 check --target loongarch64-unknown-none
```

但 RespOS 正式构建不要直接在根目录套用这两条；平台 Cargo config、用户程序嵌入和产物命名由顶层入口
管理：

```bash
make build-qemu-rv64
make build-qemu-loongarch64
make build-jh7110
make build-ls2k1000
```

四个平台共享生成的 Cargo config，必须顺序构建。

## 5. Cargo 的基本概念

### 5.1 package、crate、workspace

- package：一个 `Cargo.toml` 描述的发布/构建单位；
- crate：一次 `rustc` 编译的库或可执行单元；一个 package 可有一个 library 和多个 binary；
- workspace：用一个顶层清单组织多个 package，共享依赖解析和 `Cargo.lock`。

RespOS 的 `os/` 与 `user/` 是独立 Cargo package，各自有 lockfile；顶层 Makefile 决定构建顺序和架构
config。`vendor/` 中还有 path dependency，它们不从 crates.io 下载。

### 5.2 Cargo.toml 与 Cargo.lock

`Cargo.toml` 是人的意图：

```toml
[dependencies]
foo = "1.2"
bar = { version = "0.8", default-features = false, features = ["alloc"] }
baz = { path = "../vendor/baz" }
```

普通 `"1.2"` 不是精确版本，通常允许同一 major 下的兼容更新。`Cargo.lock` 才记录最终选中的直接和传递
版本、来源和 checksum。比赛工程应提交 lockfile，并在复验时使用：

```bash
cargo build --locked
```

若 manifest 和 lock 不一致，`--locked` 会失败而不是静默重算。`--frozen` 近似同时要求 lock 不变且完全
离线；排障时分开使用 `--locked --offline` 更容易看懂失败原因。

### 5.3 dependencies 的四种常见位置

```toml
[dependencies]          # 目标 crate 正常代码使用
[dev-dependencies]      # test/example/bench 使用
[build-dependencies]    # 在 host 上运行的 build.rs 使用
[target.'cfg(...)'.dependencies]  # 只为特定 target 加入
```

`build.rs` 在 host 上执行，即使最终目标是 `no_std` 内核，它的依赖也可能使用 `std`。不要把“build
dependency 能编译”误当成目标架构依赖也能编译。

### 5.4 feature 与 default-features

feature 是 Cargo 的条件编译开关，不是 CPU feature，也不是 Git feature branch：

```toml
[features]
default = []
trace = []

[dependencies]
serde = { version = "1", default-features = false, features = ["derive"] }
```

依赖默认会启用它自己的 default features，很多 crate 的 default 包含 `std`。内核候选依赖必须检查：

```bash
cargo tree -e features
cargo tree -i 目标crate
```

仅写 `default-features = false` 也不能保证最终关闭：另一条依赖路径可能重新启用同一个 feature。真实
RV64/LA64 编译结果比阅读一行 manifest 更可靠。

### 5.5 profile

常见 profile：

- `dev`：编译快、优化少、调试信息多；
- `release`：优化构建；
- 仓库自定义 profile：继承并覆盖选项。

profile 不等于功能 feature。不要用 release/debug 的偶然栈布局差异掩盖内存错误。RespOS 当前正式配置
保守关闭 LTO，是经项目验证的提交选择，不应因为新 crate 宣称“开启 LTO 更快”而临场修改。

## 6. `cargo add`、`cargo install`、`cargo fetch` 和 `cargo vendor`

| 命令 | 用途 | 是否修改项目 |
| --- | --- | --- |
| `cargo add foo` | 把库依赖写入 `Cargo.toml` 并解析 lock | 是 |
| `cargo install tool` | 编译并安装带可执行程序的 Cargo package | 不加入当前项目 |
| `cargo fetch` | 下载 lockfile 所需依赖到 Cargo cache | 不改源码，可能生成/更新 lock |
| `cargo vendor` | 把完整依赖源码复制到指定目录 | 生成可携带目录 |
| `cargo update -p foo` | 在约束内更新 lock 中某个包 | 修改 lock |
| `cargo tree` | 查看依赖和 feature 来源 | 否 |

因此 `serde`、`slab` 一类库不能靠 `cargo install` 给内核“安装”。应先在 manifest 中声明，Cargo 才知道
为哪个版本、target 和 feature 构建它。`cargo install` 更适合 `cargo-binutils`、`cargo-expand` 等命令行
工具，但安装的工具仍可能和固定 nightly 不兼容，必须保存版本和安装命令。

## 7. 外部依赖来源和风险

Cargo 支持：

```toml
registry_crate = "1.2"
git_crate = { git = "https://example/repo.git", rev = "完整提交号" }
path_crate = { path = "../vendor/path_crate" }
```

现场优先级建议：

```text
当前仓库已验证的 path dependency
> 当前 lockfile 已固定且离线验证的 registry crate
> 本地 vendor 中已缓存但尚未接入的 crate
> 临时联网下载的新 crate
> 未固定 rev 的 Git dependency
```

引入外部依赖前检查：

- 是否真的比几十行明确实现更安全；
- 许可证能否用于比赛提交和再分发；
- `rust-version` 是否兼容 Rust 1.86 nightly；
- 是否支持两个 `unknown-none` target；
- default feature 是否带入 `std`、线程、文件或网络；
- build script 是否需要 C/C++、bindgen、系统动态库或网络；
- 传递依赖数量和编译时间；
- `unsafe`、全局 allocator、panic、原子指令和 target-specific 汇编；
- 是否改变现有数据结构所有权、锁顺序或 ABI。

内核中新增依赖属于设计变更，不要因为“crate 已缓存”就直接采用。尤其不要临场用陌生容器替换现有
等待队列、fd table、PageCache 或 allocator。

## 8. 本次本地离线 crate 快照

2026-08-18 在当前机器的 `offline/cargo/` 建立了两个 seed package：

### 8.1 `no_std` 内核候选

```text
static_assertions 1.1.0
arrayvec 0.7.6
heapless 0.8.0
slab 0.4.9
intrusive-collections 0.9.7（alloc feature）
byteorder 1.5.0
zerocopy 0.8.48（derive feature）
```

### 8.2 宿主诊断和测试候选

```text
libc 0.2.172
goblin 0.9.3
serde 1.0.219 + serde_json 1.0.140
anyhow 1.0.97
proptest 1.6.0
loom 0.7.2
memmap2 0.9.5
tempfile 3.19.1
```

`Cargo.lock` 固定后共 vendor 93 个直接及传递 package，约 150 MiB；lockfile SHA-256 为：

```text
53eb471c0c9679d7d8e7501eea19829d39a94a10a55ea120db35bd9eb23ec64c
```

已使用空 `CARGO_HOME`、纯 vendor、`--offline --locked` 验证：

- `nightly-2025-01-18` + `riscv64gc-unknown-none-elf` kernel seed；
- `nightly-2025-01-18` + `loongarch64-unknown-none` kernel seed；
- `nightly-2025-01-18` + x86_64 Linux host seed test。

`offline/` 被 Git 忽略，所以这份快照只存在当前本地工作区和你主动复制的备份中，不属于仓库 checkout
保证。详细复验命令见 `offline/cargo/README.md`。

## 9. 完全离线使用 vendor

Cargo 的 registry cache 通常在 `$CARGO_HOME/registry`，适合同机重复构建，但它依赖用户目录布局和
registry index 状态。vendor 是更清晰的可携带快照，每个 crate 目录还包含 Cargo 使用的 checksum 文件。

把下面配置写入临时工程的 `.cargo/config.toml`：

```toml
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "/实际路径/RespOS/offline/cargo/vendor"

[net]
offline = true
```

然后执行：

```bash
cargo +nightly-2025-01-18 build --locked --offline
```

不要直接覆盖 RespOS 当前生成的 `os/.cargo/config.toml` 或 `user/.cargo/config.toml`；顶层 Makefile 会管理
其中的 target 和 linker 参数。需要在 RespOS 正式引入某个 vendor crate 时，应单独设计 source 配置或
把经过审查的 crate 作为 path dependency 纳入现有 `vendor/`，再跑完整门禁。

断网演练不能只执行 `cargo metadata`，因为 proc-macro、build script 和目标相关依赖可能到真正编译时才
暴露。至少执行两个内核 target 的 `cargo check` 和一个宿主 `cargo test`。

## 10. 在 RespOS 中新增依赖的安全流程

1. 先确认现有 `core/alloc`、仓库代码或已依赖 crate 能否完成需求；
2. 判断依赖属于 kernel、user、build script 还是宿主工具；
3. 阅读 crate 的 Cargo.toml、feature、MSRV、许可证和 unsafe 边界；
4. 在独立 seed 中固定精确版本，使用比赛 nightly 做 `no_std` 双目标检查；
5. 查看 `cargo tree -e features` 和新增传递依赖；
6. 只把需要的依赖加入 `os/Cargo.toml` 或 `user/Cargo.toml`；
7. 更新并审查对应 `Cargo.lock`，不能只提交 manifest；
8. 顺序构建 RV64 和 LA64，运行使用该依赖的最小 probe；
9. 跑相邻 subsystem 回归和 `make preflight`；
10. 记录采用原因、版本、feature、许可证和双架构证据。

不要用 `cargo update` 无差别刷新整个依赖树。若只需要更新一个包：

```bash
cargo update -p 包名 --precise 版本
git diff -- Cargo.toml Cargo.lock
```

## 11. Git 与依赖版本控制

应提交：

- `Cargo.toml`；
- `Cargo.lock`；
- 合法纳入仓库的 path dependency 源码及许可证；
- 必要的构建配置和可复现说明。

通常不提交：

- `target/`；
- 用户目录的 Cargo registry/git cache；
- `offline/` 大型缓存；
- 临时下载包和无来源压缩文件。

提交前：

```bash
git diff -- Cargo.toml Cargo.lock
git status --short
git diff --check
```

`Cargo.lock` 看起来改动很多时不要直接接受：先确认是不是工具链、registry、feature 或无差别
`cargo update` 导致了无关升级。

## 12. 高频故障诊断

### `package requires rustc X.Y`

当前依赖超过比赛工具链 MSRV。优先选择与 Rust 1.86 兼容的旧版本并精确锁定，不要只升级本地 rustc。

### `can't find crate for core/std`

对应 target 没安装到当前实际 toolchain，或者工具链选错：

```bash
rustup show active-toolchain
rustup target list --installed
rustup target add --toolchain nightly-2025-01-18 目标三元组
```

### `use of undeclared crate or module std`

某个目标依赖或其 feature 不支持 `no_std`。用 `cargo tree -e features -i 包名` 查是谁启用了 `std`。

### `failed to download ...` 或断网仍访问 registry

缓存不完整、lockfile 变化，或 vendor source replacement 未生效。使用空 `CARGO_HOME` 加
`--offline --locked` 复现，不要把临时联网成功当成离线可用。

### 本机 `cargo check` 过，`make` 不过

可能是直接 Cargo 使用了错误 target/config，或交互环境选择了较新 nightly。以：

```bash
make build-qemu-rv64
make build-qemu-loongarch64
```

为提交级入口，并检查第一段日志中的 rustc、target、feature 和 linker。

### rust-analyzer 报错但命令行能构建

先看它使用的 server、toolchain 和 target。RespOS 还存在 rust-analyzer 自动 build script 与 lwext4 CMake
目录竞争的已知问题，不能为消除编辑器红线随意升级提交工具链；详见 `docs/codex/pitfalls.md`。

## 13. 现场命令速查

```bash
# 我现在到底在用什么？
rustup show active-toolchain
rustc -Vv
cargo -V

# 强制使用比赛版本
rustc +nightly-2025-01-18 -Vv
cargo +nightly-2025-01-18 -V

# 安装/检查目标与组件
rustup target list --toolchain nightly-2025-01-18 --installed
rustup component list --toolchain nightly-2025-01-18 --installed

# 查看依赖
cargo tree
cargo tree -e features
cargo metadata --locked --format-version 1

# 离线和冻结
cargo fetch --locked
cargo build --locked --offline
cargo build --frozen

# RespOS 正式入口
make build-qemu-rv64
make build-qemu-loongarch64
make preflight
```

官方参考：

- [rustup：toolchain override](https://rust-lang.github.io/rustup/overrides.html)
- [rustup：component](https://rust-lang.github.io/rustup/concepts/components.html)
- [rustup：交叉编译 target](https://rust-lang.github.io/rustup/cross-compilation.html)
- [Cargo：manifest 与依赖](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
- [Cargo：feature](https://doc.rust-lang.org/cargo/reference/features.html)
- [Cargo：离线 vendor](https://doc.rust-lang.org/cargo/commands/cargo-vendor.html)

