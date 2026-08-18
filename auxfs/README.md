# 辅助盘资源

本目录保存 RespOS 辅助盘的源资源，不对应 guest 内的目录结构。制盘后，辅助盘根目录由内核挂载到
guest 的 `/respos`：选中的 profile 会成为 `/respos/profile`，payload 文件会出现在
`/respos/` 下。

- `profiles/`：每次制盘只选择一个运行模式；
- `payloads/software/`：软件兼容性脚本和源码；
- `payloads/bootstrap/`：不含秘密的 SSH 自举脚本和 host key。

统一使用 `scripts/build_aux_disk.sh` 组装普通辅助盘。bootstrap 私钥、SSH client 和 Rust target
只能通过 `scripts/build_bootstrap_disk.sh` 动态注入 `/tmp` 镜像，禁止放入本目录或提交到仓库。
