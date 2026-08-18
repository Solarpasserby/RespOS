# QEMU 网络使用手册

RespOS 当前在 QEMU 中使用 virtio-net 和 QEMU user networking（slirp）。QEMU 充当 NAT 路由器，宿主机
不需要配置 bridge 或 TAP，也不需要 root 权限。

```text
RespOS guest：10.0.2.15/24
QEMU 网关：   10.0.2.2
QEMU DNS：    10.0.2.3
```

当前已确认的范围是 IPv4、TCP/UDP、DNS、HTTP、Git HTTPS 和 Git SSH 自举。尚未实现或尚未完整验证的
范围包括 DHCP、IPv6 Ethernet、设备中断模式以及完整的断网重连矩阵。不要用 `ping` 作为唯一网络判据：
ICMP 不是当前网络门禁。

## 最快的出网验证

宿主机启动 RV64 软件兼容性环境：

```bash
make run-rv-software \
  RV_SOFTWARE_MEM=4G \
  RV_SOFTWARE_SMP=2
```

启动日志应出现：

```text
[net] virtio-net up, mac=..., guest ip=10.0.2.15
[contest_launcher] software mode: starting Alpine /bin/sh
```

进入 guest 的 `/ #` 提示符后运行仓库自带门禁：

```bash
sh /respos/software-network.sh
```

通过判据：

```text
SOFTWARE_NETWORK udp_dns PASS
SOFTWARE_NETWORK public_http PASS
SOFTWARE_NETWORK git_https_ls_remote PASS
SOFTWARE_NETWORK git_https_clone PASS
SOFTWARE_NETWORK ALL PASS
```

脚本实际执行 UDP DNS、公开 HTTP、GitHub HTTPS `ls-remote` 和浅克隆。它依赖宿主公网与 GitHub 可达；
离线赛场中失败不能直接归因为内核网络回归。

LA64 的对称入口：

```bash
make run-la-software \
  LA_SOFTWARE_MEM=4G \
  LA_SOFTWARE_SMP=2

# guest：
sh /respos/software-network.sh
```

RV64 与 LA64 应顺序运行，避免同时占用宿主 8080 等端口，也避免并行构建共享 Cargo 配置。

## 在 guest 中手动联网

先检查 DNS：

```bash
cat /etc/resolv.conf
```

software/final launcher 只在没有现有 `nameserver` 时安装以下 QEMU fallback：

```text
nameserver 10.0.2.3
```

如果使用旧 launcher 且文件为空，可临时设置：

```bash
printf 'nameserver 10.0.2.3\n' > /etc/resolv.conf
```

按层验证：

```bash
# UDP DNS
nslookup example.com 10.0.2.3

# 公网 HTTP
wget -O /tmp/example.html http://example.com/
head /tmp/example.html

# Git HTTPS，只读且不需要凭据
git ls-remote https://github.com/Solarpasserby/RespOS.git HEAD

# 实际克隆
cd /tmp
git clone --depth=1 https://github.com/Solarpasserby/RespOS.git
```

逐层测试的意义：DNS 失败与 TCP/HTTP 失败是不同问题；`ls-remote` 成功也不自动证明大仓库 clone、push、
SSH、断线恢复等路径全部正确。

## 从宿主机访问 RespOS

诊断构建会启用仅用于本地诊断的内核 HTTP 服务。终端一运行：

```bash
make run-rv-diagnostic \
  RV_DIAGNOSTIC_MEM=4G \
  RV_DIAGNOSTIC_SMP=2
```

应看到：

```text
[net] virtio-net up, mac=..., guest ip=10.0.2.15
[net] in-kernel HTTP server listening on port 80
```

保持 QEMU 运行，在宿主机终端二执行：

```bash
curl -sS http://127.0.0.1:8080/ \
  -o /tmp/respos-http.html \
  -w '%{http_code}\n'
```

预期状态码为 `200`。数据路径为：

```text
宿主 127.0.0.1:8080
→ QEMU hostfwd
→ guest 10.0.2.15:80
→ RespOS 内核 HTTP 服务
```

QEMU user networking 后面的 `10.0.2.15` 通常不能由宿主直接连接。要让宿主访问 guest listener，必须
增加 `hostfwd`。LA64 对称入口为：

```bash
make run-la-diagnostic \
  LA_DIAGNOSTIC_MEM=4G \
  LA_DIAGNOSTIC_SMP=2

# 宿主另一终端：
curl -sS http://127.0.0.1:8080/
```

该内核 HTTP 服务只用于 RX/TX、ARP、TCP listen/accept 和状态观测，不是正式交付功能。普通
preliminary/final/software/submission 构建不会占用 guest 80 端口。

## Makefile 中的 QEMU 网络参数

RV64：

```text
-device virtio-net-device,netdev=net
-netdev user,id=net,hostfwd=tcp::8080-:80
```

LA64：

```text
-device virtio-net-pci,netdev=net0
-netdev user,id=net0,hostfwd=tcp::8080-:80,...
```

其中：

- `virtio-net-device` / `virtio-net-pci` 是呈现给 RespOS 的虚拟网卡；
- `-netdev user` 启用无需特权的 QEMU NAT；
- guest 主动访问公网不需要 `hostfwd`；
- `hostfwd=tcp::8080-:80` 只负责“宿主 8080 → guest 80”的入站转发；
- RV64 使用 virtio-mmio，LA64 使用 virtio-pci，用户态 socket 行为保持一致。

内核没有通过 DHCP 获取地址；`os/src/net/mod.rs` 为 QEMU user network 配置静态地址、默认路由和网卡
选择。物理板卡或不同 subnet 不能直接复用这些常量。

## 故障分流

### 没有 `virtio-net up`

检查日志是否出现：

```text
[net] virtio-net not present; real network disabled
```

这表示内核仍可使用 `127.0.0.1` loopback，但不能出网。优先改用仓库现成的 `run-*-software` 或
`run-*-diagnostic`，并核对 QEMU 是否包含对应 virtio-net device，不要先修改 socket 实现。

### DNS 失败

```bash
cat /etc/resolv.conf
nslookup example.com 10.0.2.3
```

若直接指定 `10.0.2.3` 成功、按域名访问仍失败，问题位于 resolver 配置；若指定 DNS 也超时，再查
virtio-net、UDP、QEMU NAT 和宿主网络。

### DNS 成功但 HTTPS 失败

```bash
date
ls -l /etc/ssl/certs/ca-certificates.crt
git ls-remote https://github.com/Solarpasserby/RespOS.git HEAD
```

检查系统时间、CA bundle、TCP 连接和 TLS 用户态依赖。禁止使用关闭证书校验的选项掩盖错误。

### 宿主访问 8080 失败

宿主检查：

```bash
ss -ltnp | grep ':8080'
```

确认没有另一个 QEMU 占用端口，并确认 guest 日志已出现内核 HTTP 监听成功。端口转发只在 QEMU 进程
存活时有效。

### 所有公网请求失败

先在宿主验证外网：

```bash
curl -I http://example.com/
git ls-remote https://github.com/Solarpasserby/RespOS.git HEAD
```

宿主也失败时优先检查赛场网络、DNS、代理或沙箱限制。宿主成功而 guest 失败时，再保存完整 QEMU 日志，
从 `[net] virtio-net up`、DNS、TCP connect 和首个 errno 依次定位。

## 当前实现入口

| 层次 | 位置 |
| --- | --- |
| QEMU 设备与端口转发 | `Makefile` 的 `run-rv-qemu` / `run-la-qemu` |
| 网卡驱动 | `os/src/drivers/virtio/net_dev.rs` |
| Ethernet 与 smoltcp 适配 | `os/src/net/ethernet.rs` |
| 静态 IPv4、路由和接口选择 | `os/src/net/mod.rs` |
| TCP/UDP socket | `os/src/net/tcp.rs`、`os/src/net/udp.rs`、`os/src/net/socket.rs` |
| Linux socket syscall 边界 | `os/src/syscall/net.rs` |
| DNS fallback 和启动模式 | `user/src/bin/contest_launcher.rs` |
| 可复现网络门禁 | `auxfs/payloads/software/software-network.sh` |

更完整的验证证据和实现不变量见
[`docs/codex/workflows.md`](../codex/workflows.md#virtio-net-用户态网络门禁与内核内-http-诊断) 与
[`docs/codex/architecture.md`](../codex/architecture.md#smoltcp-socketset-由-loopback-与-virtio-net-两个接口共同驱动)。
