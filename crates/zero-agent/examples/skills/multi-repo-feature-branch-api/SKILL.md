---
name: multi-repo-feature-branch-api
description: 识别多仓库创建 feature 分支意图，校验参数后返回结构化 JSON。需要目标分支名、源分支名和目标模式。
allowed-tools:
  - json_validate
---

## Overview

当用户请求在多个仓库同时创建统一的 feature 远程分支时使用此 skill。从用户的自然语言中提取目标分支名、源分支名和目标模式，校验后输出结构化 JSON 供 lark-bot 执行。

## Input Schema

```json
{
  "type": "object",
  "required": ["targetBranch", "sourceBranch"],
  "properties": {
    "targetBranch": {
      "type": "string",
      "minLength": 1,
      "description": "目标分支名（要创建的 feature 分支），不能为空"
    },
    "sourceBranch": {
      "type": "string",
      "minLength": 1,
      "description": "源分支名（从哪个分支拉出），不能为空"
    },
    "targetMode": {
      "type": "string",
      "enum": ["功能开发", "生产发布", "生产内测"],
      "default": "功能开发",
      "description": "目标模式，可选值：功能开发、生产发布、生产内测。用户未指定时默认为「功能开发」"
    }
  },
  "additionalProperties": false
}
```

## Validation Rules

1. 检查 `targetBranch` 是否存在且非空字符串。
2. 检查 `sourceBranch` 是否存在且非空字符串。
3. 如果用户未提供 `targetMode`，自动使用默认值 `功能开发`，无需向用户询问。
4. 如果用户提供了 `targetMode` 但不在枚举值内，输出 `status: "need_info"` 提示可选值。
5. 如果有必填参数缺失，输出 `status: "need_info"` 的 JSON，在 `message` 中列出缺少的字段。
6. 全部参数就绪且合法后，输出 `status: "ready"` 的 JSON。

## Tool Usage

在输出最终 JSON 之前，必须先调用 `json_validate` 工具校验 `params` 是否符合上方的 Input Schema：
- 如果校验通过（Valid），继续输出完整响应 JSON
- 如果校验失败（Invalid），根据错误信息修正参数后重新校验，直到通过为止

## Output Format

**严格约束：你的响应必须有且仅有一个裸 JSON 对象。禁止在 JSON 前后输出任何文字、解释、思考过程或 markdown 格式。不要用代码块包裹。**

三种 JSON 格式之一：

### 参数就绪

```json
{
  "status": "ready",
  "action": "create_branch",
  "params": {
    "targetBranch": "<目标分支名>",
    "sourceBranch": "<源分支名>",
    "targetMode": "<功能开发 | 生产发布 | 生产内测>"
  }
}
```

### 需要补充信息

```json
{
  "status": "need_info",
  "action": "create_branch",
  "message": "<面向用户的自然语言提示，说明缺少什么、格式要求是什么>"
}
```

### 请求超出范围

```json
{
  "status": "rejected",
  "message": "我当前只处理三类事务：\n1. App 打包构建\n2. 多仓库统一创建 git feature 分支\n3. 工单转发\n这个请求不在我的授权范围内。"
}
```

## Examples

### 示例 1：参数完整

用户输入：「帮我从 develop 拉一个 feature/login-v2 分支，功能开发用」

输出：

{"status":"ready","action":"create_branch","params":{"targetBranch":"feature/login-v2","sourceBranch":"develop","targetMode":"功能开发"}}

### 示例 2：未指定目标模式（使用默认值）

用户输入：「从 develop 拉一个 zeroclaw_develop_test 分支」

输出：

{"status":"ready","action":"create_branch","params":{"targetBranch":"zeroclaw_develop_test","sourceBranch":"develop","targetMode":"功能开发"}}

### 示例 3：目标模式不在枚举中

用户输入：「从 develop 拉 feature/test 分支，用来做测试」

输出：

{"status":"need_info","action":"create_branch","message":"「测试」不是有效的目标模式。请从以下选项中选择：功能开发、生产发布、生产内测。"}

### 示例 4：缺少全部参数

用户输入：「帮我创建一个分支」

输出：

{"status":"need_info","action":"create_branch","message":"需要以下信息才能创建分支：\n1. 目标分支名（要创建的分支）\n2. 源分支名（从哪个分支拉出）\n请提供以上信息。目标模式默认为「功能开发」，如需其他模式请一并说明（可选：生产发布、生产内测）。"}

### 示例 5：多轮对话补充参数

用户上一轮输入：「从 develop 拉一个 zeroclaw_develop_test 分支」（已回复 ready）
用户本轮输入：「功能开发」

输出：

{"status":"ready","action":"create_branch","params":{"targetBranch":"zeroclaw_develop_test","sourceBranch":"develop","targetMode":"功能开发"}}
