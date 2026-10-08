# Equipment retargeting / 装备骨架重定向

## 使用方式

v2.55.0 支持自动检测玩家装备的骨架比例差异。

装备加载后，DLL 自动读取 FLVER 蒙皮绑定中的参考骨架，与玩家动画参考骨架比较同名身体骨段的长度。检测到比例差异时，使用玩家动画驱动目标模型，保留模型自己的骨长与蒙皮绑定，并校准手臂参考姿态差异。首轮针对本地玩家的全身替换装备。

**默认自动检测，无需填写装备名字、骨骼倍率或新增 `[retarget]` 配置。** 全局 `enabled` 和 `[player].enabled` 仍需开启。已有整体缩放配置可以继续使用；启用重定向不需要 SpEffect 规则。

检测使用参考姿态，不使用正在播放的动作。仅改变头发、衣摆等附加骨骼不会触发身体比例重定向；仅改变参考旋转也不会触发长度检测。长度比较使用少量浮点容差，避免导出舍入误差造成误判。没有同名身体骨段可比较时，保留原流程。判断结果按模型资源缓存，模型重建或换装时重新检查。

## 模型与物理资产

- 基础关节名与玩家对应，名称唯一，父子关系有效。

- FLVER 保留新模型的参考姿态，蒙皮绑定与该姿态一致；不要在导出时把骨长烘焙回玩家原版比例。

- 布料物理资产是可选的。没有 `_c.hkx`／`_c.clm2` 的装备也可以使用网格参考骨架进行重定向；无骨骼部件保留原生显示。

- 头发、衣服物理资产的参考骨架、绑定、形状、约束长度和碰撞体按目标模型制作。

DLL 将目标姿态传给装备自身的物理输入，保留游戏自由段模拟，再采用物理写回结果显示头发和衣服。它不负责将旧比例物理资产自动变形。玩家共享骨架与玩法碰撞保持原有数据。完整身体模型按参考腿长适配动画实际位移，按参考头部高度适配普通跟随镜头；指定镜头挂点的动作保留原生镜头。武器和收纳挂点跟随重定向后的身体，完整的同名手指关节用于校准新旧手掌的握持区域，避免沿用原版手腕到握点的距离；武器大小和原动画握持方向保持。双手握持按当前动画的两手关系及两侧手掌尺寸求解，保持目标骨长。脚部在落地且原生脚部 IK 启用时沿用原生接触高度和脚面方向，修正新腿长下的接触。修正后的同一份姿态同时用于装备显示和物理输入。

初次验证建议使用整体倍率 1.0，检查新模型比例与物理后，再验证整体缩放组合、换装和不同动作。重复骨名、循环层级、不支持的内存布局或无法表示的姿态会拒绝该次绑定或输出。卸下装备、重建模型或离开存档会撤销旧绑定。

## 可选控制

普通使用不需要以下表。需要排除特殊模型或强制重定向时，可在已有 `ERCharacterScale.toml` 最后添加：

```toml

[retarget]

enabled = true

exclude_models = ["BD_M_KEEP_NATIVE"]

force_models = ["BD_M_FORCE_RETARGET"]

```

名字为游戏加载的内部模型名，大小写不敏感，可能不同于装备道具编号和外部文件编号。同一名字不能同时强制和排除。强制选项跳过比例差异判定，但不跳过骨架与内存校验。早期开发版的 `models` 字段等同于 `force_models`。

设置 `[retarget] enabled = false` 可关闭重定向。配置在启动时读取，修改后重启游戏。[最小示例](../examples/ERCharacterScale.retarget.toml) 不包含任何必填装备列表。

## English

v2.55.0 supports automatic detection of different player-equipment proportions.

**Automatic detection is enabled by default. No equipment list or `[retarget]` table is required.** Keep the existing global and player switches enabled. Retargeting does not require a SpEffect rule and can coexist with overall scaling rules.

At equipment binding, the adapter compares matching body-chain lengths from the player's reference skeleton and the loaded FLVER skin binding. When proportions differ, it transfers animation to the target while preserving authored lengths and skin binding while calibrating arm reference-pose differences. Detection uses reference geometry, not the current animation. Changes limited to hair/cloth bones or reference rotations do not trigger the length detector. A small numerical tolerance ignores export rounding noise. Models without comparable body chains retain the native path. Decisions are cached until the binding changes.

The initial intended use is a local-player full-body replacement. Preserve matching unique base-joint names, valid hierarchy, target reference proportions and consistent skin binding. Author the equipment's physics skeleton, reference shape, constraints and colliders for those proportions. The adapter supplies a private target pose to native equipment physics and uses simulated output for rendering. It does not reshape old physics assets or modify shared gameplay animation/collision data. For a complete body, animation-driven displacement follows the reference leg-length ratio and normal camera follow height uses the reference head-height ratio. Explicit camera dummy overrides retain native behavior. Weapons and sheaths follow the retargeted anchors. Matching finger landmarks calibrate the animated grip region for the new palm dimensions while preserving weapon size and native grip orientation. Two-handed grip uses the current animation’s relative trajectory, both palms’ calibration and length-preserving IK. Grounded foot correction uses native contact height and ankle orientation while native foot IK is enabled. Rendering and equipment physics consume the same corrected pose.

The optional table above can exclude a model or force retargeting despite unchanged detected lengths. Use internal loaded model names, matched case-insensitively; names may differ from item IDs and file numbers. Conflicting overrides are rejected. Forcing does not bypass data validation. The earlier development field `models` is an alias for `force_models`. Set `enabled = false` inside `[retarget]` to disable retargeting. Restart after configuration changes.

Validate first at overall scale 1.0, then test scale combinations, equipment changes and different actions. Unsupported bindings fall back to native behavior; unequipping or rebuilding revokes stale bindings. Automated geometry and owned-memory tests do not establish native cloth dynamics, GPU output or game-frame performance. These remain in-game acceptance items.

Cloth physics assets are optional. Equipment without `_c.hkx` / `_c.clm2` files can retarget through its mesh reference skeleton. Parts with no bones retain native rendering.

## 验证边界 / Validation scope

斜坡修正不会新增地图射线检测；它沿用原生脚部的局部接触平面。台阶边缘、极端比例、超出新手臂可达范围的握点，以及特殊武器动作仍需实机验收。无法到达的目标不会通过伸长骨骼满足。带中间辅助关节或非均匀缩放的肢体链保留普通重定向。多槽位不同体型的接缝协调不在本次实现中。

Foot correction transfers the native local contact plane; it does not issue new terrain raycasts. Stair edges, extreme proportions, unreachable grips and special weapon animations require in-game validation. Unreachable targets do not stretch authored bones. Unsupported intermediate-joint or nonuniformly scaled limb chains retain ordinary retargeting. Cross-slot seam coordination remains outside this implementation.

## 身体挂点 / Body dummy points

本地玩家的通用身体挂点会随全身装备的新骨架移动，支持同一 Dummy ID 的多个挂点与批量读取。挂点相对骨段的距离按对应参考骨长适配，特效的矩阵尺寸保持不变。武器挂点继续使用手掌握点校准，避免重复修正。未绑定到可对应身体骨骼的挂点保留原行为；多个全身重定向装备同时生效、无法确定身体归属时不接管通用挂点。此适配依据骨架，不是从网格表面重新生成挂点。

Common local-player body dummy points follow the retargeted full-body skeleton, including repeated IDs and batch queries. Bone-relative distances adapt to reference bone lengths without resizing the effect matrix. Weapon queries retain their separate palm calibration and are corrected once. Unmapped points and ambiguous multiple full-body bindings retain native behavior. This is skeleton-based adaptation, not reconstruction of attachment points from the mesh surface.

## 手臂姿态 / Arm posture

上臂和前臂会自动校准源、目标参考姿态的方向差，相关扭转辅助骨保留自己的动画运动。目标肩宽、臂长和蒙皮绑定保持原值；双手握持在校准后的姿态上求解。不需要填写固定外张角。无法确定方向、参考比例非均匀或基础关节链不完整时保留原参考映射。

Upper arms and forearms automatically compensate for reference-pose direction differences. Related twist branches keep their animated motion. Authored shoulder width, arm lengths and skin binding remain intact; two-handed constraints run after calibration. No fixed outward angle is required. Unsupported or ambiguous chains retain the original reference mapping.

Generic dummy selectors used by temporary props and effects pass through body retargeting. Nested suppression is limited to a valid registered weapon consumer, which applies its own palm correction once. Unmapped auxiliary points retain native behavior; effect particle dimensions are not changed.
