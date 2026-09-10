# ERCharacterScale

Elden Ring 玩家与敌人体型缩放 DLL，通过 TOML 配置效果触发或无条件倍率。

当前源码为 **2.54.0-rc.1 候选版本**，运行标识为 `er-2.54-configurable-units-rc1`。
配置和多单位数值检查已自动验证，游戏中的敌人外观及动作仍待实测。
已发布稳定版 **2.53.0** 已在 **BD9004、0.5 倍缩放**场景完成验收，继续保留用于对照。

## 支持范围

- Windows x64，离线 Elden Ring **WW2.7.1.0 / game1.17.1**。
- 本地玩家、c0000 敌对 NPC，以及 **非 c0000 的 EnemyIns 角色模型**；没有 c0000 模型限制。
- 敌人按原版与本地玩家的敌对关系识别，不要求进入战斗；友好/中立 NPC、远程玩家、骨灰、幽灵、坐骑不作为敌人目标。
- 整体等比缩放，范围0.5～3.0；不逐骨骼修改，也不关闭布料模拟。
- DLL检查原版程序及关键函数布局；不兼容的程序版本不会启用缩放任务。
- 倍率相对于每个单位自己的原始大小。不同单位分别保留姿态、碰撞和恢复状态。

普通敌人、带布料敌人、非人形敌人和 Boss 是本轮覆盖目标，尚未逐种完成游戏验收。
特殊模型布局验证失败时保持原比例并记录原因。若几个实例共用可修改的布料数据，
相同倍率复用原始基准；要求不同倍率时，这组实例保持原比例，以避免互相改写。
该候选版不复制或重新分配游戏的 Havok 资源，因而这类共享冲突是明确的限制。
攻击判定范围、抓取、处决和特殊 Boss 阶段的完整等比兼容也尚未证明。

## 使用

把候选包的 `player_scale_no_bone.dll` 和 `ERCharacterScale.toml` 放在同一目录，
让现有 DLL 加载器加载 DLL。修改配置后重启游戏生效。配置缺失时自动生成兼容默认文件；
错误配置会禁用该次启动的缩放，并在同目录日志中写明原因。

默认配置保留下面的玩家 SpEffect 映射，**敌人缩放默认关闭**。
要直接测试非 c0000 敌人，使用包内 `examples/ERCharacterScale.enemies-half.toml`
替换 DLL 旁的 `ERCharacterScale.toml`：玩家继续使用旧映射，所有支持的敌对单位
无条件缩为 0.5 倍，无需给敌人添加 SpEffect。

`examples/ERCharacterScale.fixed.toml` 则让玩家固定 0.75 倍、敌人固定 0.5 倍。
自定义 SpEffect、模型/NpcParam/实体筛选和规则顺序详见 [配置说明](docs/CONFIGURATION.md)。
示例必须复制到 DLL 同目录并命名为 `ERCharacterScale.toml` 才会被读取。

稳定版下载仍在 [Releases](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.53.0)；
2.53 的 DLL不读取新的 TOML，也不具备敌人功能。

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

普通日志写入 DLL 同目录，`[ERCS-UNIT]` 记录单位模型、NpcParam、实体、命中规则和
请求/实际倍率；`[ERCS-ENEMIES]` 记录敌人路线检查结果。已有玩家一次性 JSONL
诊断保留，采样结束不影响规则继续生效；它不能替代敌人的游戏验收。

## 从源码构建

需要Rust nightly及Windows x64 MSVC构建环境（Visual Studio C++ Build Tools / Windows SDK）。
已验证编译器：`rustc 1.98.0-nightly (f428d123a 2026-06-19)`。

本地候选交付附有同版本的源码 ZIP；解压后在其项目根运行 `cargo build --release --locked`。
下面的克隆命令获取 GitHub 公开 main；候选尚未发布时，main 仍是 2.53 稳定版。

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

稳定版发布包保留已完成游戏验收的原始 DLL。文件信息见 [2.53 发布清单](release/v2.53.0.json)。
候选版复用这条修正链路，并把归属和倍率分配到各自的单位实例。

## License

本项目使用 [MIT License](LICENSE)。依赖和参考代码保留各自的许可证及署名，
见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
