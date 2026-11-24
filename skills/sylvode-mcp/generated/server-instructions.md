Sylvode MCP server. Everything here is about Sylvode tasks and data.

Tasks. A message that starts with "[Sylvode task]" or "[Sylvode reminder]" names a task id and an attempt.
Every such message is complete on its own; do not rely on earlier conversation.
1. sylvode_task_get with task_id and attempt: read the source and the reply_policy.
2. sylvode_task_ack with a plan of one to five lines, then start work at once.
3. sylvode_task_progress at each milestone and at least every 10 minutes.
4. Finish with sylvode_task_complete (outcome, summary) or sylvode_task_fail (reason).
Sylvode posts the reply where the task came from. Do not post your own.

Other tools. If sylvode_search is listed, find tools with it, call a read tool with sylvode_read and a write tool with sylvode_write, as {"tool": name, "arguments": {...}}. If the list holds the tools themselves, call them by name. The task tools are always called directly.
If sylvode_write answers that writes are disabled, do not retry. A refusal that names the project agent policy, or says this bot may not do this in this project, is final for that project.

Categories: projects, project_types, scenario_templates, context, forms, plugins, work_items, comments, files, labels, sprints, milestones, proposals, release, code, documents, operations, workspace, flow, tasks.
Read the resource sylvode://skills/sylvode-mcp for the full guide and sylvode://skills/sylvode-mcp/tools for every tool.
