# 配置体型缩放

适用于 2.54.0-rc.4。默认 DLL 名称为 `ERCharacterScale.dll`。文件是 DLL 同目录的 `ERCharacterScale.toml`，
使用 UTF-8 编码，可以带 BOM。启动时读取一次；修改文件后重启游戏。
配置中的 SpEffect 获得或失去仍会在游戏运行时改变倍率。

## 无条件缩放

下面是一个可以直接使用的完整配置：玩家 0.75 倍，敌人 0.5 倍。
`target = "enemy"` 是兼容名称，现指支持的本地非玩家单位，包括非 c0000 模型、友方和召唤物。

```toml
version = 1
enabled = true

[player]
enabled = true

[enemies]
enabled = true

[[rules]]
name = "fixed-player"
target = "player"
mode = "constant"
scale = 0.75

[[rules]]
name = "fixed-enemies"
target = "enemy"
mode = "constant"
scale = 0.5
```

`constant` 不查询 SpEffect，不能同时填写 `sp_effect_id`。
若只需要缩放敌人，将 `[player]` 的 `enabled` 设为 `false`。
整体停用则将文件开头的 `enabled` 设为 `false`。

## 按 cxxxx 指定型号

例如 c9520 固定为 0.6 倍，在启用 `[enemies]` 后添加：

```toml
[[rules]]
name = "c9520"
target = "enemy"
character_ids = [9520]
mode = "constant"
scale = 0.6
# hostile_only = true  # 仅需要限制为敌对单位时才添加
```

同型号的友方和召唤物也会匹配。要分别缩放其他型号，另写一条不同名称和 `character_ids` 的规则。其他型号不受这条规则影响。现有 c9520 配置无需改名或添加字段。

## SpEffect 和单位筛选

每个单位从文件上方开始，使用第一条完整匹配的规则。规则不会相乘或累加。
效果检测来自该单位自身；玩家和敌人可以对相同效果配置不同倍率。
以下规则可以添加到完整配置的开关之后；示例 ID 是占位值，需要换成实际值。

```toml
[[rules]]
name = "selected-enemy-effect"
target = "enemy"
mode = "speffect"
sp_effect_id = 8020400
scale = 1.5
character_ids = [1234]       # 模型 c1234；这里仅演示数字写法
npc_param_ids = [123456]    # 目标的 NPC_PARAM_ST 行号
entity_ids = [12345678]     # 地图事件实体编号，用于指定实例

[[rules]]
name = "enemy-default"
target = "enemy"
mode = "constant"
scale = 0.8
```

同一个列表内是“任一满足”，不同筛选字段之间是“全部满足”。省略筛选表示不限制该字段；
空列表是错误配置。只按模型筛选时删去另外两个字段。模型编号不含 `c` 前缀，
`character_ids = [0]` 才表示仅 c0000；没有这个字段时不会限制为 c0000。
`entity_ids` 不接受 0 或 4294967295 这两个未指定占位值。

效果规则没有触发时会继续匹配后面的规则，因此固定倍率的兜底规则应放在最后。
放在最前面的无筛选固定规则会优先命中，后面的同类规则将不会被采用。
没有规则命中时恢复到该单位自己的原始大小。

## 字段和错误

| 字段 | 含义 |
| --- | --- |
| `version` | 当前必须为 1 |
| `enabled` | 总开关；`[player]` 和 `[enemies]` 内另有各自的开关 |
| `name` | 每条规则唯一且非空的名称，日志使用这个名称 |
| `target` | `player`（本地玩家）或 `enemy`（支持的本地非玩家单位，兼容名称） |
| `hostile_only` | 可选布尔值，默认 `false`；设为 `true` 时仅匹配与玩家敌对的单位，不能用于玩家规则 |
| `mode` | `speffect` 或 `constant` |
| `sp_effect_id` | 非负整数，仅用于 `speffect` 模式 |
| `scale` | 有限数值 0.5～3.0，1.0 为该单位原始大小 |
| `character_ids` | 可选的角色模型编号列表 |
| `npc_param_ids` | 可选的 NPC 参数行编号列表 |
| `entity_ids` | 可选的地图事件实体编号列表 |

未知字段、重复名称、错误类型、非法倍率、空筛选列表和模式冲突会禁用该次启动的缩放。
默认构建保持静默，不输出日志；诊断构建将错误写入 `ERCharacterScale.log`。错误配置不会悄悄换回默认规则或截断倍率。
配置缺失时使用兼容默认值并尝试创建文件；已有文件不会被覆盖。

## 敌人的当前边界

默认按规则的模型、NPC 参数或实体编号匹配，不查询阵营。友好/中立 NPC、调试生成单位及召唤物可以命中同一型号的规则。联机/远程玩家、幽灵和坐骑仍排除；模型类型或必要结构无法验证时，该单位保持原比例。

`hostile_only = true` 才调用原版敌对关系判断，与是否进入战斗无关。非敌对或关系无法确认时跳过该条规则，继续匹配后面的规则；没有后续匹配则恢复原大小。这与 rc.1/rc.2 强制排除友方的行为不同，若需要旧行为，在非玩家规则中显式添加该字段。

各实例分别保留原始尺寸。游戏若让多个实例共用布料尺寸数据，相同比例可以共用原始基准；
不同比例会使这组实例保持 1.0，并记录 `shared-cloth-consumers-require-different-scales`。
这包括一个被选中的敌人与一个没有被选中、必须维持原大小的单位共用数据的情况。
对照物和友军因此不会被间接缩放。其他独立资源的单位继续生效。
布料资源在原实例上改变布局而无法安全复用基准时，会记录 `cloth-resource-layout-changed`。

该版本已通过非 c0000 模型归属、姿态、普通碰撞及布料修正链路的离线检查。
普通敌人、非人形、带布料和 Boss 的具体外观、动作、阶段变化需要逐个在游戏中验收。
本功能不承诺伤害、移动速度、抓取对位或全部攻击判定范围随倍率改变。
