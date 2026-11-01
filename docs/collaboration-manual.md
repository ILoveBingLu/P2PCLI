# 两设备协作手动闭环

更新日期：2026-10-27。本文对应当前源码中的 `p2pcli collab` 命令，提供可执行的手动操作步骤，不是一份实机验收报告。

核心存储、网络协议、CLI 和桌面入口已有实现；协作测试中的两个节点主要在同一进程内运行。managed worker 当前只有 mock 子进程/JSON 等自动化测试，没有完成真实 AI 长任务或物理两设备 E2E 验收。测试数量不作为本文的能力承诺。设计边界与完整验收要求见 [协作设计](cross-device-collaboration.md)。

## 桌面连线入口

2026-11-01：协作入口改为设备侧栏连线，终端保留在主区域。原项目、成员和投递管理仍可从协作详情的「高级管理」进入。

1. 从设备或会话行右侧的连接点拖到另一行，也可以依次点击两个连接点；Esc 取消。不同设备之间、同一设备的不同会话之间均可连接，不允许连接到自身。
2. 若目标已登记可用的协作会话，确认执行会话并填写任务目标。设备有多个可用会话时必须显式选择；直接连到终端时，按设备、终端 ID 和终端 owner 精确匹配，不会改投其他会话。
3. 未登记或心跳失效时显示待接入。「接入会话」可选择单个 AI 终端或「全部会话」，头像组最多显示 5 个图标，但接入范围包含全部选中终端。「连接协作」会连接设备、打开对应会话并自动提交各自的接入请求，不再向输入草稿追加内容。首次新增项目成员仍需管理员明确勾选执行者授权；不自动恢复已撤销成员或提升观察者权限。
   自动提交要求对端支持 `agent_message_v1`，并在终端所在设备重新确认 AI 类型、进程、空闲状态、空输入提示符及 bracketed paste 支持。忙碌、授权菜单、已有输入或无法识别的提示符会停止提交；不自动批准 AI 权限。只提交一次，服务进程存续期间按请求标识去重，输入开始后的不确定结果不会自动重发。程序最多等待两分钟核对真实会话登记、终端所属者和心跳，发送成功不等于接入成功。批量接入逐项核实，失败会报告具体会话；没有把失败会话当作可用成员。复制按钮保留为不支持自动接入时的手动入口。
   登记、心跳与任务领取仍由交互 AI 使用本机 CLI 执行，凭据不返回桌面。这不是通用后台托管器；AI 退出、停止心跳或未主动领取时不会继续执行任务。服务重启后去重缓存不保留，重试前仍先查询已有登记，未确认的请求需先检查终端。
4. 接入成功后重新连线分配任务。任务进度、结果、验收和停止请求在右侧详情查看。「待领取」不表示 AI 已经执行；关闭抽屉或取消画线不取消任务。

### 全部会话与主会话

连接到设备时，执行会话可选择「全部会话」，范围是该设备已登记、心跳有效且仍获授权的会话，不包含尚未接入的终端。头像组最多显示 5 个会话图标，其余显示数量；显示上限不限制实际参与名单。直接连接指定终端仍只分配给该终端。

此模式只创建一个主任务，不向所有会话复制相同任务。连线起点的可用管理员/调度者会话担任主会话；存在多个候选时需显式选择。主会话未登记、不可用或没有调度权限时禁止提交，不自动提升权限。提交前重新核对主会话、参与会话和成员权限；提交后的重试沿用原请求与名单。

主任务 `inputs.p2pcli_team` 包含完整参与名单及调度要求。主会话领取后，使用已有 CLI 将需求拆成独立子任务，按预计工作量、已有负载和依赖关系分配，协调冲突写入，并汇总真实结果。子任务通过 `inputs.p2pcli_parent` 中的主任务 ID、需求 ID 和预计工作量关联。最终主任务仍由用户验收。

这是交给 AI 主会话执行的协作约定，不是后端自动拆解器或强制公平调度器；主会话必须正在运行且主动拉取/领取任务。应用不自动唤醒 AI、不发送终端输入，当前也没有完成真实多 AI 自动拆分和汇总的端到端验收。父子关联是输入元数据，取消主任务不会由服务端自动级联取消子任务；主会话需要按调度要求停止分配、取消子任务并确认停止，主会话离线时应由管理员检查和处理仍在执行的子任务。

`/preview.html?s=collab-team` 提供 7 个参与会话的 UI 模拟，`collab-team-no-lead` 提供缺少可用主会话的阻止提交场景。
`collab-onboarding` 和 `collab-onboarding-many` 分别模拟 2 个、7 个待接入的 CodeBuddy 会话，用于检查授权、自动提交、登记确认以及头像上限。`collab-onboarding-busy`、`collab-onboarding-old`、`collab-onboarding-timeout` 覆盖部分忙碌、旧版对端及登记超时；这些模拟不替代真实 AI 和两设备测试。

无需构建的验证：`node --experimental-strip-types --test scripts/check-collaboration-links.test.mjs` 检查接入确认、权限、超时和去重标识；`node scripts/check-agent-input.mjs` 在独立临时命名空间中测试仓库自带的 tmux/psmux、定向粘贴、中文和多会话隔离，不启动 P2PCLI。后者目前已在 Windows 运行，macOS 和真实 AI 接入仍需实际验收。

默认使用本机的已有项目；有多个项目或远端协调设备时，从「协作范围」切换。没有本机项目时，在明确准备接入或确认任务的写操作中创建「设备协作」，单纯拖线和取消不会创建项目。

连线起点作为任务输入中的来源元数据保存，不冒充起点 AI 已授权发起任务，也不自动复制终端内容或 AI 内部上下文。实际创建者仍是当前认证设备。会话登记凭据仅保存在执行设备本机。

前端预览使用 `npm run dev:web` 后的 `/preview.html?s=collab`；`collab-unregistered`、`collab-multiple`、`collab-viewer`、`collab-device-offline`、`collab-stale`、`collab-queued`、`collab-lost-reply` 分别提供未接入、多会话、只读权限、设备离线、心跳过期、待投递和响应丢失场景。这些是 UI 模拟，不是物理两设备验收。

## 1. 准备两台设备

设备 A 是协调设备和验收方，设备 B 是执行方。两台设备都需要运行包含协作实现的桌面应用，并使用相应版本的 CLI。CLI 调用本机控制服务，不会自行启动桌面应用，也不会自动唤醒已有 AI 会话。

先在 B 的桌面应用中连接 A，完成正常的设备认证和授权。确认双方的已知设备列表能够定位对方：

```sh
p2pcli --json devices
p2pcli collab --help
```

从对方设备记录的 `endpoint_id` 获取完整公钥。设备信任与项目成员授权是两层检查，连接成功后仍需执行下面的 `member set`。

如果从源码构建 CLI，在仓库根目录运行：

```sh
cargo build -p p2pcli-cli
```

然后将下文的 `p2pcli` 替换为该构建的可执行文件路径：macOS/Linux 为 `./target/debug/p2pcli`，Windows 为 `.\target\debug\p2pcli.exe`。若本机应用使用自定义数据目录，CLI 的 `--data-dir` 或 `P2PCLI_DATA_DIR` 必须指向同一个目录。

以下命令用一行表示，适用于 shell 或 PowerShell。大写标记均需替换，不能原样执行：

| 标记 | 来源 |
| --- | --- |
| `A_DEVICE_ID`、`B_DEVICE_ID` | 两台设备的完整公钥 |
| `PROJECT_ID` | 创建项目响应的 `response.project.id` |
| `B_SESSION_ID` | B 登记响应的 `response.session.id` |
| `TASK_ID` | 创建任务响应的 `response.task.id` |
| `ATTEMPT_ID` | 领取响应中 `response.task.attempts` 最后一项的 `id` |
| `TASK_VERSION` | 检查任务后得到的 `response.task.version`，作为数字传入 |

每台设备使用一个固定的本地操作目录，下面的相对文件路径都以它为基准。凭据文件放在仓库之外。A 的命令省略 `--coordinator` 表示本机；B 始终使用同一个完整 `A_DEVICE_ID`。凭据校验要求协调设备作用域一致，不要交替使用设备名、缩写、公钥和 `local`。

## 2. A 创建项目并授权 B

在 A 执行：

```sh
p2pcli --json collab project create --name "两设备验证"
```

记下 `PROJECT_ID`，再执行：

```sh
p2pcli --json collab --project "PROJECT_ID" member set "B_DEVICE_ID" --role worker
p2pcli --json collab --project "PROJECT_ID" member list
p2pcli --json collab --project "PROJECT_ID" --session-file ./a-session.json session register --name "A 验收会话" --mode manual-pull
```

创建项目的设备拥有管理员权限；B 获得该项目的 worker 权限。当前没有 `project join` 命令，成员不能自行加入或提权。

登记返回会话信息与 `response.credential_file`，token 仅写入本机私有凭据文件。不要打印文件内容、把它写进日志、提交到 Git 或传给另一台设备。重新登记一个新会话时使用新文件名；同一个凭据文件不能被另一个会话覆盖。

## 3. B 登记执行会话

在 B 执行：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json session register --name "B 手动验证会话" --mode manual-pull
```

记下 `B_SESSION_ID`。在 A 可查询已登记会话：

```sh
p2pcli --json collab --project "PROJECT_ID" session list
```

这里登记的是协作身份，不会创建终端或启动 AI。使用已有 AI 会话时，需要由用户明确授权它读取任务、领取和执行。可用状态由会话报告，不是进程存活的保证；B 可更新标记：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json session heartbeat --available true
```

## 4. A 创建并分配任务

在 A 准备 UTF-8 的 `task.json`。以下是对 P2PCLI 仓库做验证的模板，仓库与基线必须填写真实值，B 需要已有相同代码或另行取得代码：

```json
{
  "title": "验证指定基线的协作测试",
  "goal": "在 B 的独立工作目录核对基线，运行指定测试并回报真实输出；不修改代码。",
  "acceptance": "基线一致，提供命令、退出码和失败详情，由 A 检查后验收。",
  "inputs": {
    "repository": "REPOSITORY_ID_OR_URL",
    "baseline_commit": "FULL_COMMIT_SHA",
    "commands": [
      "git rev-parse HEAD",
      "git status --short",
      "cargo test -p p2pcli-core --test collaboration"
    ]
  },
  "assignee": {
    "device_id": "B_DEVICE_ID",
    "session_id": "B_SESSION_ID"
  }
}
```

`inputs` 是自由 JSON；系统不会自动检查基线、同步仓库、建立 worktree 或处理未提交改动。任务确实依赖未提交代码时，需显式交付补丁并记录输入身份，不能只写 commit。

在 A 执行：

```sh
p2pcli --json collab --project "PROJECT_ID" --session-file ./a-session.json task create --file ./task.json
```

保存 `TASK_ID`。正常响应应有 `response.type = "task"`、`response.task.state = "queued"`；创建时已经指定执行会话，无须再执行 `task assign`。只有 `delivery = "queued_local"` 时，请求尚待同步，还没有可确认的远端任务结果。

## 5. B 领取、执行和回报

在 B 查询分配给本设备的任务：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task list
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task show "TASK_ID"
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task claim "TASK_ID"
```

确认响应中任务为 `running`，最新 attempt 的设备和会话属于自己，再记录 `ATTEMPT_ID` 并开始执行。领取响应丢失时先按第 8 节重放原请求，不要生成另一次领取并直接执行。

核对本地仓库、基线、工作目录和未提交改动后，再运行任务中实际授权的命令。过程中可提交进度：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task progress "TASK_ID" --attempt "ATTEMPT_ID" --message "已领取，正在核对本地基线。"
```

需要人工处理时，用准确说明加 `--blocked`；文件形式为 `--file ./progress.txt`，其内容是 UTF-8 文本，不是进度 JSON。

执行后准备 `result.json`，以真实结果替换下面的占位说明：

```json
{
  "summary": "填写实际结论",
  "completed": ["填写实际完成项"],
  "remaining": [],
  "verification": ["填写实际命令、基线、退出码及关键输出"],
  "artifact_ids": [],
  "failed": false
}
```

如果执行失败，把 `failed` 改为 `true`，并填写失败原因和未完成项。需要提交日志产物时，先完成第 7 节的声明，再把产物 ID 加到 `artifact_ids`；不要仅在摘要里声称文件已经交付。

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task result "TASK_ID" --attempt "ATTEMPT_ID" --file ./result.json
```

JSON 结果不能与 `--completed`、`--verification`、`--artifact`、`--failed` 等结果详情参数混用。成功结果进入 `result_ready`，明确失败进入 `failed`，执行进程退出不会自动完成验收。

## 6. A 检查并验收

在 A 执行：

```sh
p2pcli --json collab --project "PROJECT_ID" task wait "TASK_ID" --timeout 60 --poll-ms 1000
p2pcli --json collab --project "PROJECT_ID" task show "TASK_ID"
p2pcli --json collab --project "PROJECT_ID" artifact list "TASK_ID"
```

`wait` 等到结果或终态才返回，超时不会取消任务。检查最新 attempt 的结果、验证证据及产物；只有满足验收条件且状态为 `result_ready` 时，使用刚检查的版本：

```sh
p2pcli --json collab --project "PROJECT_ID" task accept "TASK_ID" --version TASK_VERSION
```

预期任务进入 `completed`。`--version` 用于防止检查后发生并发变更；冲突时重新读取和判断，不能盲目覆盖。省略版本的修改会先读取一次当前版本再提交，也仍可能冲突。

A 的原 AI 会话结束后，新的获授权会话可重新登记并读取任务、输入、结果和产物，再继续协调或验收。执行中的 attempt 仍绑定 B 原会话；换会话、离线或超时不会自动转移执行权。

## 7. 可选：日志产物和消息

### 日志产物

在 B 提交结果之前，将真实日志保存为 `verification.log`，计算其字节数和 **BLAKE3** 的 64 位十六进制摘要。可使用另行安装的 BLAKE3 工具；SHA-256 不能代替。将下列 `FILE_BYTES` 替换为实际非负整数、`BLAKE3_HEX` 替换为真实摘要：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json artifact declare "TASK_ID" --attempt "ATTEMPT_ID" --name verification.log --size FILE_BYTES --hash "BLAKE3_HEX"
p2pcli --json send "A_DEVICE_ID" ./verification.log
```

声明响应的 `response.artifact.id` 是 `ARTIFACT_ID`。文件传输使用既有授权流程，等待接收完成。然后在 A 查询收到文件的实际路径，并本机登记接收：

```sh
p2pcli --json inbox
p2pcli --json collab --project "PROJECT_ID" artifact receipt "ARTIFACT_ID" --file "ABSOLUTE_PATH_ON_A"
```

`artifact receipt` 必须在协调设备本机执行，不传 `--coordinator`。路径是 A 上真实收到的文件，不是 B 的源路径。接收入口核验大小和 BLAKE3；B 的结果引用此产物 ID 后，A 验收还会核验产物。传输与登记是两个步骤，中断后先查记录、再补登记，不能将“文件已发送”等同于“任务已验收”。

### 会话消息

在 A 发补充消息：

```sh
p2pcli --json collab --project "PROJECT_ID" --session-file ./a-session.json message send --device "B_DEVICE_ID" --session "B_SESSION_ID" --task "TASK_ID" --message "请在结果中说明实际测试平台。"
```

B 主动拉取并确认已读，将 `MESSAGE_ID` 替换为返回消息的 `id`：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json inbox
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json message consume "MESSAGE_ID"
```

`collab inbox` 是会话消息，`p2pcli inbox` 是文件收件箱，待领取任务在 `task list` 中。消息快照落盘后会确认投递，`consume` 才表示会话明确消费；两者都不代表领取任务，也不会自动执行消息里的命令。首次成功拉取后，本机服务会保留收件订阅并后台同步，AI 仍需主动读取。

## 8. 重试、断线与取消

每次写操作会输出 `request_id` 和私有 `request_file`。网络超时或响应丢失时，在原设备用原文件重放；B 的示例：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" request --file "ORIGINAL_REQUEST_FILE"
```

A 原本省略协调设备的请求，重放时同样省略。文件已经保存项目、凭据和原条件版本，不要编辑它或把它交给另一台设备。也可显式用 `--request-id`，但同一 ID 必须对应同一操作与相同内容。业务版本冲突处理后要提交新的决定，应使用新的请求 ID。

```sh
p2pcli --json collab deliveries
```

`deliveries` 查询本机投递记录，不接受协调设备、项目、会话或请求 ID 参数。后台自动重试范围是 `TaskCreate` 和 `MessageSend`；领取、进度、结果等由调用方明确重试，managed worker 另有自身执行记录和发件逻辑。

| 输出 | 含义 |
| --- | --- |
| `delivery: "queued_local"` | 请求仅在本机排队，未确认协调设备已提交 |
| `delivery: "committed_unverified"` | 本机记录过提交结果，但当前无法重新核实远端权限或状态 |
| `delivery: "unknown"` / 传输错误 | 可能尚未提交，也可能提交后响应丢失；重放原请求核实 |
| `response.task.state: "result_ready"` | 执行方已提交成功结果，仍待验收 |
| `response.task.state: "completed"` | 协调设备已记录验收 |

不能仅凭退出码 0 判断任务成功。普通协作请求中，1 表示业务/运行错误，2 表示 Clap 参数错误，3 表示不支持能力；`wait` 超时为 4，等到 `failed` / `cancelled` 也可能正常退出，需检查状态。

A 可请求取消：

```sh
p2pcli --json collab --project "PROJECT_ID" task cancel "TASK_ID"
```

尚未领取可直接取消；已运行进入 `cancel_requested`。B 必须先真实停止执行，再确认：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-session.json task stop "TASK_ID" --attempt "ATTEMPT_ID"
```

`task stop` 是停止确认，不会替手动会话终止进程。取消不回滚已经发生的文件修改或外部动作。成员撤销也不能收回已交付的数据或保证离线进程立即停止。

事件查询使用 `events --after CURSOR --limit 100`。当前为立即分页读取，调用方处理完成后保存返回的 `response.cursor`；它不是服务端长轮询。

## 9. managed worker：实验性入口

手动流程不需要启动 worker。当前适配器面向 Claude Code 的非交互 JSON 接口，是独立的前台 CLI 进程，桌面服务仍需运行；不会自动接管已有交互会话。mock 测试使用测试子进程输出 JSON，不能证明真实 AI 长任务、权限等待、取消、跨平台和两设备端到端行为已经可靠。

试用时先在 B 登记一个独立会话，不与手动执行共享会话：

```sh
p2pcli --json collab --coordinator "A_DEVICE_ID" --project "PROJECT_ID" --session-file ./b-worker-session.json session register --name "B 实验 worker" --mode managed-adapter
```

用该新会话 ID 作为任务 assignee。`worker.json` 示例：

```json
{
  "coordinator": "A_DEVICE_ID",
  "project_id": "PROJECT_ID",
  "credential_file": "./b-worker-session.json",
  "cwd": "/absolute/path/to/local/worktree",
  "program": "claude",
  "poll_interval_secs": 5
}
```

`cwd` 必须是 B 上实际存在、已经准备好的工作目录；Windows JSON 路径可用 `C:/work/project`。相对目录和凭据路径以配置文件目录为基准，凭据仍由登记命令产生，不手填 token。

```sh
p2pcli collab worker --config ./worker.json
```

worker 的协调设备、项目和凭据来自配置，不同时传 `--coordinator`、`--project`、`--session-file` 或 `--request-id`。当前启动代码使用 `-p --output-format json --permission-mode default --permission-prompts none`，这是实现参数，不代表已验证用户安装的真实 AI 版本支持全部行为。真实环境试用前仍需核验版本和权限流程。

worker 保存本地 attempt、结果、日志和待发记录；重启时无法确认完成情况的已启动 attempt 需要人工核实，不自动重跑外部动作。测试代码和 CLI 帮助可用于检查接口，但物理两设备、真实 AI 长任务及异常恢复验收应另记设备、版本、输入、实际结果与失败证据。
