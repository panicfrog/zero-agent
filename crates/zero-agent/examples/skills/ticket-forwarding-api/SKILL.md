---
name: ticket-forwarding-api
description: 识别工单转发意图，校验参数后返回结构化 JSON。需要工单号和接收人邮箱，转发人信息由系统自动获取。
allowed-tools:
  - json_validate
---

## Overview

当用户请求转发工单时使用此 skill。从用户的自然语言中提取工单号和接收人邮箱，校验后输出结构化 JSON 供 lark-bot 执行。转发人身份由 lark-bot 从飞书会话中自动获取，无需用户提供。

## Input Schema

```json
{
  "type": "object",
  "required": ["ticketId", "receiverEmail"],
  "properties": {
    "ticketId": {
      "type": "string",
      "minLength": 1,
      "description": "工单号，不能为空"
    },
    "receiverEmail": {
      "type": "string",
      "format": "email",
      "description": "接收人邮箱，必须为合法邮箱地址"
    }
  },
  "additionalProperties": false
}
```

## Validation Rules

1. 检查 `ticketId` 是否存在且非空字符串。
2. 检查 `receiverEmail` 是否存在且为合法邮箱格式（包含 `@` 和域名部分）。
3. 如果有任何参数缺失或格式不正确，输出 `status: "need_info"` 的 JSON，在 `message` 中列出缺少或不合法的字段及其要求。
4. 全部参数就绪且合法后，输出 `status: "ready"` 的 JSON。
5. 如果用户主动提供了自己的邮箱，忽略即可，不要放入 params。

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
  "action": "forward_ticket",
  "params": {
    "ticketId": "<工单号>",
    "receiverEmail": "<接收人邮箱>"
  }
}
```

### 需要补充信息

```json
{
  "status": "need_info",
  "action": "forward_ticket",
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

用户输入：「把工单 T20250317001 转给 bob@za.com」

输出：

{"status":"ready","action":"forward_ticket","params":{"ticketId":"T20250317001","receiverEmail":"bob@za.com"}}

### 示例 2：缺少接收人

用户输入：「帮我转发工单 T20250317001」

输出：

{"status":"need_info","action":"forward_ticket","message":"缺少接收人邮箱。请提供工单接收人的邮箱地址。"}

### 示例 3：接收人邮箱格式不正确

用户输入：「把工单 T20250317001 转给 bob」

输出：

{"status":"need_info","action":"forward_ticket","message":"接收人邮箱格式不正确，需要完整的邮箱地址（如 name@za.com）。请重新提供。"}

### 示例 4：缺少全部参数

用户输入：「帮我转发一下工单」

输出：

{"status":"need_info","action":"forward_ticket","message":"需要以下信息才能转发工单：\n1. 工单号\n2. 接收人邮箱\n请提供以上信息。"}
