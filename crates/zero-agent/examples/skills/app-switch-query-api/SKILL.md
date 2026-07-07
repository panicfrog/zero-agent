---
name: app-switch-query-api
description: 识别 App 开关查询意图，提取开关名称后返回结构化 JSON。
allowed-tools:
  - json_validate
---

## Overview

当用户请求查询某个 App 开关的状态时使用此 skill。从用户的自然语言中提取开关名称（switch code），校验后输出结构化 JSON 供 lark-bot 执行查询。

## Input Schema

```json
{
  "type": "object",
  "required": ["switchName"],
  "properties": {
    "switchName": {
      "type": "string",
      "minLength": 1,
      "description": "开关名称（switch code），不能为空"
    }
  },
  "additionalProperties": false
}
```

## Validation Rules

1. 检查 `switchName` 是否存在且为非空字符串。
2. 如果缺少开关名称，输出 `status: "need_info"` 的 JSON，在 `message` 中说明需要提供开关名称。
3. 参数就绪后，输出 `status: "ready"` 的 JSON。

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
  "action": "app_switch_query",
  "params": {
    "switchName": "<开关名称>"
  }
}
```

### 需要补充信息

```json
{
  "status": "need_info",
  "action": "app_switch_query",
  "message": "<面向用户的自然语言提示，说明缺少什么>"
}
```

### 请求超出范围

```json
{
  "status": "rejected",
  "message": "我当前只处理 App 开关查询，这个请求不在我的授权范围内。"
}
```

## Examples

### 示例 1：参数完整

用户输入：「查一个开关 ZAAppV3LiquidButton」

输出：

{"status":"ready","action":"app_switch_query","params":{"switchName":"ZAAppV3LiquidButton"}}

### 示例 2：缺少开关名称

用户输入：「帮我查一下开关」

输出：

{"status":"need_info","action":"app_switch_query","message":"请提供需要查询的开关名称（switch code）。"}

### 示例 3：自然语言描述开关名

用户输入：「ZAAppV3LiquidButton 这个开关现在是什么状态」

输出：

{"status":"ready","action":"app_switch_query","params":{"switchName":"ZAAppV3LiquidButton"}}
