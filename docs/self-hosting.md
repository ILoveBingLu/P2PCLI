**简体中文** | [English](self-hosting.en.md)

# 自建牵线服务器和中继

软件默认用 `wss://p2pcli.aiqji.cn` 牵线，不自建也能用。想用自己的，在软件的 设置 → 连接设置 里换掉。

## 牵线服务器

牵线服务器只负责发设备码、交换地址、转达 IPv6 发包通知，终端数据不经过它。有两种接法，客户端按地址写法自动区分：

| 写法 | 通道 | 适合 |
| --- | --- | --- |
| `公钥@IP:端口` 或 `公钥@域名:端口` | UDP（iroh，加密并认证服务器身份） | 自己有公网服务器 |
| `wss://域名` | WebSocket（TLS） | 放在 Cloudflare Workers 这类只支持 HTTP 的平台，不用自己维护服务器 |

两种接法连的是不同的服务器时，设备之间互相查不到；同一台自建服务器同时开两个入口时，走 UDP 和走 WebSocket 的设备能互相查到。

### 自建服务器

Linux 服务器（带 systemd，x86_64 或 aarch64）上一条命令装好：

```sh
curl -fsSL https://raw.githubusercontent.com/ILoveBingLu/P2PCLI/main/crates/server/install.sh | sudo bash
```

脚本会下载发布页上编译好的程序、核对校验和、装成开机自启的服务，最后打印软件里要填的地址（`公钥@IP:端口`）。装完在云服务器控制台的安全组里放行 UDP 7788。再跑一次就是升级（只换程序，原来的服务配置不动），参数写在 `bash -s --` 后面，例如 `| sudo bash -s -- --port 9000`：`--ws-port` 另开 WebSocket 入口，`--mirror` 给国内服务器下载 GitHub 慢时加代理前缀，`--uninstall` 卸载，`--help` 看全部。

不用脚本也可以直接跑：

```sh
p2pcli-server --data-dir /var/lib/p2pcli --port 7788 --ws-port 7789 --public-ip <公网IP>
```

启动时会打印 UDP 写法要填的地址（`公钥@IP:端口`），IP 也可以换成直接解析到服务器的域名，每次连接时现解析，服务器换 IP 改解析就行。`--ws-port` 开的是明文 WebSocket，对外用 `wss://` 时在前面加反向代理（例如 Caddy：`rdv.example.com { reverse_proxy 127.0.0.1:7789 }`），或者把域名放在 Cloudflare、阿里云 ESA 这类 CDN 后面。CDN 只转 HTTP，域名走了 CDN 就只能用 `wss://` 写法，好处是服务器 IP 不公开，CDN 帮着挡攻击。

### Cloudflare Workers

`cloudflare/` 文件夹是同一套协议的 Workers 版，单独一个文件夹，一条命令就能部署：

```sh
cd cloudflare
./deploy.sh 你的域名        # Windows 用 .\deploy.ps1 你的域名
```

需要一个托管在 Cloudflare 的域名，部署后客户端填 `wss://你的域名`。详细说明、本机试跑和费用见 [cloudflare/README.md](../cloudflare/README.md)。

## 中继

打不通时转发终端数据的中继要另外准备，iroh 官方公共中继在国内很慢甚至连不上。中继是 [iroh-relay](https://github.com/n0-computer/iroh)，需要一台有公网 IP 的服务器：

```sh
iroh-relay --config-path relay.toml   # 用 Let's Encrypt 自动签证书
```

放行 TCP 80/443（中继）、UDP 7842（中继的地址探测）。客户端在设置里填中继地址，或用 `P2PCLI_RELAY`。
