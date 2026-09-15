# ERCharacterScale

**简体中文** | [English](README.en.md)

通过 TOML 配置 Elden Ring 玩家与其他角色的整体体型比例，支持 SpEffect 触发和无条件缩放。

[下载 v2.54.3](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.54.3) · [配置说明](docs/CONFIGURATION.md) · [版本记录](CHANGELOG.md)

## 功能

- 玩家和非玩家单位分别配置，倍率相对于各单位自己的原始大小。
- 支持玩家、c0000 本地 NPC 和非 c0000 角色模型。
- 按 `cxxxx` 模型编号、NPC 参数编号或事件实体编号筛选，分别设置不同倍率。
- 使用单位自身的 SpEffect 触发缩放，或通过 `constant` 模式始终应用倍率。
- 保留布料模拟。

## 安装

适用于 **Windows x64、离线 Elden Ring WW2.7.1.0 / game1.17.1**。其他程序版本需要另行验证。

1. 从 [Release 页面](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.54.3) 下载 `ERCharacterScale-v2.54.3-windows-x64.zip`。
2. 将 `ERCharacterScale.dll` 和 `ERCharacterScale.toml` 放在同一目录，并使用现有 DLL 加载器加载该 DLL。
3. 按需要编辑 TOML，然后启动游戏。之后修改配置文件，需要重启游戏生效。

升级时退出游戏后替换 DLL，保留自己的 `ERCharacterScale.toml`。包中的 TOML 是默认配置；默认启用玩家效果规则，**非玩家缩放默认关闭**。

发布的 DLL 已内置所需的 C/C++ 运行库，无需额外安装 Visual C++ Redistributable 或 Rust。Windows 系统组件由操作系统提供。

## 配置

配置文件为 DLL 同目录的 [ERCharacterScale.toml](ERCharacterScale.toml)，使用 UTF-8 编码。

- `[player]` 和 `[enemies]` 分别控制玩家和非玩家单位。
- `target = "enemy"` 表示支持的本地非玩家单位，包括友方和召唤物；只有添加 `hostile_only = true` 才限制为敌对单位。
- `character_ids = [9520]` 用于匹配 c9520。不同模型可分别写规则。
- 每个单位采用文件中第一条完整匹配的规则；规则不会叠乘，未匹配时恢复该单位的原始大小。
- SpEffect 规则跟随该单位获得或失去效果而改变倍率；`constant` 模式不需要 SpEffect。

可从以下完整示例开始，复制为 DLL 旁的 `ERCharacterScale.toml` 后按需修改：

- [固定倍率](examples/ERCharacterScale.fixed.toml)：玩家 0.75 倍，支持的非玩家单位 0.5 倍。
- [非玩家单位减半](examples/ERCharacterScale.enemies-half.toml)：保留默认玩家效果规则，支持的非玩家单位固定 0.5 倍。

筛选字段、规则顺序和配置错误处理见 [完整配置说明](docs/CONFIGURATION.md)。

## 从源码构建

需要 Rust nightly 和 Windows x64 MSVC 工具链（Visual Studio C++ Build Tools / Windows SDK）。

```powershell
git clone https://github.com/KamiyamaShiki0704/ERCharacterScale.git
cd ERCharacterScale
cargo build --release --locked
```

输出为 `target/x86_64-pc-windows-msvc/release/ERCharacterScale.dll`。仓库已配置静态 C/C++ 运行库链接。Cargo 从 Git 获取 `fromsoftware-rs`，固定提交为 `02fa5681e27fd2ecd9da79d34aef9fae96805539`；仓库不包含该依赖源码或 Git 子模块。`Cargo.lock` 固定依赖解析结果。

开发与自动检查入口见 [验证说明](docs/VALIDATION.md)。

## License

本项目使用 [MIT License](LICENSE)。依赖和参考代码的许可证及署名见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
