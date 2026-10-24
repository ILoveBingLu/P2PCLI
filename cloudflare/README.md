# P2PCLI 牵线服务 · Cloudflare Workers 版

P2PCLI 的牵线服务（发设备码、交换地址、转达 IPv6 发包通知），部署在 Cloudflare Workers 上，不用自己维护服务器。终端数据不经过这里。

软件默认用的就是部署在 `wss://p2pcli.yscopy.cn` 的这一份。想用自己的，照下面部署后在软件 设置 → 牵线服务器 里填 `wss://你的域名`。

## 一键部署

准备：

- 一个 Cloudflare 账号，账号里托管着一个域名（`workers.dev` 在国内被污染，必须用自己的域名）
- Node.js 18 以上和 pnpm（`npm i -g pnpm`）

在这个文件夹里运行：

```sh
# macOS / Linux
./deploy.sh p2pcli.example.com

# Windows（PowerShell）
.\deploy.ps1 p2pcli.example.com
```

不写域名就部署到 `p2pcli.yscopy.cn`。第一次运行会打开浏览器登录 Cloudflare，之后不用再登录。脚本会自动绑定这个子域名并签证书，最后检查能不能访问。

只想确认环境没问题、不真正部署：`./deploy.sh --check`（Windows 用 `.\deploy.ps1 -Check`）。

## 本机试跑

```sh
pnpm install
pnpm run dev          # 跑在 http://127.0.0.1:8787
```

软件或命令行的牵线服务器填 `ws://127.0.0.1:8787`（局域网里别的电脑填 `ws://本机IP:8787`）。

## 费用

所有设备连到同一个 Durable Object，设备码存在它自带的 SQLite 里。几十个人、每人几台设备的用量在 Workers 免费额度内。

## 协议

和 Rust 版牵线服务器（`crates/server`）完全一样，见 `crates/core/src/rdv_ws.rs`：

- 连接 `/v1/ws` 时请求头 `x-p2pcli-auth` 带 `设备公钥.时间戳.签名`，签的是 `p2pcli-rdv-ws-v1|主机名|设备公钥|时间戳`（Ed25519，时间戳前后 5 分钟内有效）
- 设备发 `{"id":1,"req":{...}}`，服务回 `{"id":1,"resp":{...}}`，主动推送 `{"push":{...}}`

| 文件 | 作用 |
| --- | --- |
| `src/index.ts` | Worker 和 Durable Object 代码 |
| `wrangler.jsonc` | Workers 配置 |
| `deploy.sh` / `deploy.ps1` | 一键部署脚本 |
