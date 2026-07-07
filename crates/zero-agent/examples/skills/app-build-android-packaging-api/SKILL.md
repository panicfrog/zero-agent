---
name: app-build-android-packaging-api
description: 识别 Android 打包构建意图，校验参数后返回结构化 JSON。需要构建类型、分支名和测试环境。
allowed-tools:
  - json_validate
---

## Overview

当用户请求触发 Android 打包构建时使用此 skill。从用户的自然语言中提取构建类型、分支名和测试环境，校验后输出结构化 JSON 供 lark-bot 执行。构建过程耗时较长（最多 60 分钟），由 lark-bot 后台处理并通过飞书消息推送进度，此 skill 只负责触发。

## Input Schema

```json
{
  "type": "object",
  "required": ["branch", "environment"],
  "properties": {
    "BUILD_TYPE": {
      "type": "string",
      "enum": ["Staging", "Develop", "Staging01", "Debug", "Release", "Gamma"],
      "description": "构建类型，可选值：Staging、Develop、Staging01、Debug、Release、Gamma。用户未指定时：environment 为 sit 默认为 Staging，为 uat 默认为 Gamma"
    },
    "branch": {
      "type": "string",
      "minLength": 1,
      "description": "分支名，不能为空"
    },
    "environment": {
      "type": "string",
      "enum": ["sit", "uat"],
      "description": "测试环境，可选值：sit、uat"
    },
    "hotfixBase": {
      "type": "boolean",
      "default": false,
      "description": "是否构建 React Native hotfix 的 base 包，默认为 false。仅在 environment 为 uat 时有效，sit 环境忽略此字段"
    }
  },
  "additionalProperties": false
}
```

## Validation Rules

1. 如果用户未提供 `BUILD_TYPE`，按 `environment` 取默认：`sit` 为 `Staging`，`uat` 为 `Gamma`，无需向用户询问。
2. 如果用户提供了 `BUILD_TYPE` 但不在枚举值内，输出 `status: "need_info"` 提示可选值。
3. 检查 `branch` 是否存在且非空字符串。
4. 检查 `environment` 是否存在且为 `sit` 或 `uat`，缺失时必须向用户询问。
5. `hotfixBase` 仅在 `environment` 为 `uat` 时有意义：
   - 用户未提供时默认为 `false`，无需询问。
   - 若 `environment` 为 `sit`，则输出中不包含 `hotfixBase` 字段。
6. 如果有任何参数缺失或不合法，输出 `status: "need_info"` 的 JSON，在 `message` 中逐一列出问题及要求。
7. 全部参数就绪且合法后，输出 `status: "ready"` 的 JSON。

## Tool Usage

在输出最终 JSON 之前，必须先调用 `json_validate` 工具校验 `params` 是否符合上方的 Input Schema：
- 如果校验通过（Valid），继续输出完整响应 JSON
- 如果校验失败（Invalid），根据错误信息修正参数后重新校验，直到通过为止

## Output Format

**严格约束：你的响应必须有且仅有一个裸 JSON 对象。禁止在 JSON 前后输出任何文字、解释、思考过程或 markdown 格式。不要用代码块包裹。**

三种 JSON 格式之一：

### 参数就绪（sit 环境）

```json
{
  "status": "ready",
  "action": "build_android",
  "params": {
    "BUILD_TYPE": "<构建类型>",
    "branch": "<分支名>",
    "environment": "sit"
  }
}
```

### 参数就绪（uat 环境）

```json
{
  "status": "ready",
  "action": "build_android",
  "params": {
    "BUILD_TYPE": "<构建类型>",
    "branch": "<分支名>",
    "environment": "uat",
    "hotfixBase": false
  }
}
```

### 需要补充信息

```json
{
  "status": "need_info",
  "action": "build_android",
  "message": "<面向用户的自然语言提示，说明缺少什么、格式要求是什么>"
}
```

### 请求超出范围

```json
{
  "status": "rejected",
  "message": "我当前只处理三类事务：\n1. App 打包构建（需指定平台：iOS 或 Android）\n2. 多仓库统一创建 git feature 分支\n3. 工单转发\n\n这个请求不在我的授权范围内。"
}
```

## Examples

### 示例 1：sit 环境，参数完整

用户输入：「帮我打包 Android Staging 环境，分支 feature/login-v2，sit 环境」

输出：

{"status":"ready","action":"build_android","params":{"BUILD_TYPE":"Staging","branch":"feature/login-v2","environment":"sit"}}

### 示例 2：uat 环境，不构建 hotfix base 包

用户输入：「打包 Android Release 版本，master 分支，uat 环境」

输出：

{"status":"ready","action":"build_android","params":{"BUILD_TYPE":"Release","branch":"master","environment":"uat","hotfixBase":false}}

### 示例 3：uat 环境，构建 hotfix base 包

用户输入：「uat 环境，Android，master 分支，需要构建 hotfix base 包」

输出：

{"status":"ready","action":"build_android","params":{"BUILD_TYPE":"Gamma","branch":"master","environment":"uat","hotfixBase":true}}

### 示例 4：缺少 environment

用户输入：「帮我打包 Android Staging 环境的 feature/test 分支」

输出：

{"status":"need_info","action":"build_android","message":"缺少测试环境。请指定测试环境，可选值为 sit 或 uat。"}

### 示例 5：构建类型不在枚举中

用户输入：「打包 Android Production 版本，master 分支，sit 环境」

输出：

{"status":"need_info","action":"build_android","message":"「Production」不是有效的构建类型。请从以下选项中选择：Staging、Develop、Staging01、Debug、Release、Gamma。"}

### 示例 6：未指定构建类型（使用默认值）

用户输入：「帮我打包 Android feature/login-v2 分支，uat 环境」

输出：

{"status":"ready","action":"build_android","params":{"BUILD_TYPE":"Gamma","branch":"feature/login-v2","environment":"uat","hotfixBase":false}}

### 示例 7：缺少全部参数

用户输入：「帮我打个 Android 包」

输出：

{"status":"need_info","action":"build_android","message":"需要以下信息才能打包构建：\n1. 分支名\n2. 测试环境（sit 或 uat）\n请提供以上信息。构建类型未指定时：sit 环境默认为 Staging，uat 环境默认为 Gamma；如需其他类型请一并说明（可选：Develop、Staging01、Debug、Release、Gamma）。"}
