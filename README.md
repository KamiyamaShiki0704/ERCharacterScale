# ERCharacterScale

Elden Ring 本地玩家等比缩放 DLL，支持身体、装备姿态、布料和普通角色碰撞同步缩放。

当前版本 **2.53.0**，运行标识为 `er-2.53-rigid-collider-velocity`。
已在 **BD9004、0.5 倍缩放**场景确认持续抽搐问题消失。

## 支持范围

- Windows x64，离线 Elden Ring **WW2.7.1.0 / game1.17.1**。
- 当前仅作用于本地玩家；尚未实现敌人/NPC缩放。
- 整体等比缩放，范围0.5～3.0；不逐骨骼修改，也不关闭布料模拟。
- DLL检查原版程序及关键函数布局；不兼容的程序版本不会启用缩放任务。

## 使用

从 [Releases](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases) 下载
`ERCharacterScale-v2.53.0-windows-x64.zip`，解压后让现有的DLL加载器加载
`player_scale_no_bone.dll`。使用原有事件、脚本或效果管理工具为本地玩家添加下表
对应的SpEffect。DLL本身不提供菜单、快捷键或效果编辑器。

未匹配到表中效果时倍率恢复为1.0；多个效果同时存在时采用表中最先匹配的一项。
效果8020400对应0.5倍，8020401对应3.0倍。

| SpEffect ID | 倍率 |
| --- | --- |
| `21202000` | 1.05 |
| `21202001` | 1.10 |
| `21202002` | 1.15 |
| `21202003` | 1.20 |
| `21202004` | 1.25 |
| `21202005` | 1.30 |
| `21202050` | 0.95 |
| `21202051` | 0.90 |
| `21202052` | 0.85 |
| `21202053` | 0.80 |
| `21202054` | 0.75 |
| `21202055` | 0.70 |
| `8020400` | 0.50 |
| `8020401` | 3.00 |

2.53保留自动诊断：普通日志和一次性JSONL采样写入DLL所在目录，采样会自行结束。
诊断文件不影响后续正常运行，不属于源码仓库内容。

## 从源码构建

需要Rust nightly及Windows x64 MSVC构建环境（Visual Studio C++ Build Tools / Windows SDK）。
已验证编译器：`rustc 1.98.0-nightly (f428d123a 2026-06-19)`。

```powershell
git clone https://github.com/KamiyamaShiki0704/ERCharacterScale.git
cd ERCharacterScale
cargo build --release --locked
```

输出为 `target/release/player_scale_no_bone.dll`。仓库不是fsrs工作区的子项目，
没有Git子模块，也不包含fsrs源码。Cargo自行获取以下固定提交：

```toml
eldenring = { git = "https://github.com/KamiyamaShiki0704/fromsoftware-rs", rev = "02fa5681e27fd2ecd9da79d34aef9fae96805539" }
fromsoftware-shared = { git = "https://github.com/KamiyamaShiki0704/fromsoftware-rs", rev = "02fa5681e27fd2ecd9da79d34aef9fae96805539", package = "fromsoftware-shared" }
```

`Cargo.lock`也固定了依赖解析结果。这里的依赖使用完整提交SHA，不跟随分支最新版本。

## 测试

```powershell
cargo test --locked
cargo test --release --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt -- --check
```

默认测试不需要游戏文件。7项真实采样回放测试及1项原版程序检查默认标为ignored；
它们需要使用者自己的本地数据，不随仓库分发。详见 [验证说明](docs/VALIDATION.md)。

## 2.53 修复内容

碰撞体的目标旋转带有体型缩放，而原版上一帧旋转会归一化。两者混用时，
矩阵转四元数的速度计算会产生持续反向振荡。2.53在原版计算前，验证角色归属，
通过临时副本去除旋转轴中的已知缩放，同时保留位置、填充位及原版物理调用。

发布包使用已完成游戏验收的原始DLL。文件信息见 [发布清单](release/v2.53.0.json)。

## License

本项目使用 [MIT License](LICENSE)。依赖和参考代码保留各自的许可证及署名，
见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
