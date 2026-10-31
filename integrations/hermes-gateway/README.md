# Hermes 原生项目传输

复用已有 Project Gateway 的任务、交付、回执与验收接口，只替换所选项目的执行传输。其他项目继续走原 A2A；不修改或重启 Hermes/微信共享网关，不另建服务。

原生流程：核对官方桌面会合记录与进程 → 核对/创建业务项目 → 在 `协作办公区/大肥鱼/hud-<request_key>` 创建明确 cwd 的原生会话 → 核对项目、cwd 和配置的模型 → 持久保存会话映射 → 唯一发送任务。

本项目派工优先使用 Mercury 3 私有网关（provider=`mercury3-belmo`、model=`mercury-3`），`reasoning_effort=high` 通过官方 `session.create` 参数显式传入。实际连接由本机 `state/native-desktop.json` 配置；凭据仍由 Hermes 保存，不进入仓库。

传输中断后只重新读取同一持久会话。已保存提交映射而回执未知时不重发提示词。未提交模型前的准备错误记为 failed；失败任务只允许记录 rejected。完成与验收保持分离。

`desktop_rpc.cjs` 使用 Node 24 内置 fetch/WebSocket，以 JSONL 服务于原 Python Gateway；凭据只在本机内存，不写日志。连接不代答审批。关闭 stdin 后正常释放连接。Gateway 同时保留原聊天的 delivery/ack/review 流程。

部署前运行 `python -m unittest discover -s . -p 'test_*.py'` 和 `node --test desktop_rpc.test.cjs`。`deploy.ps1` 核对原始源码和进程身份、保存数据库/源码备份，通过现有 `_shutdown` 正常退出空闲 Gateway 后原位更新。它不关闭 Hermes 桌面、微信或其他执行端；服务不空闲时退出。回滚源码保存在输出的 backup 目录，Git 基线为 a145ec0。

配置 `state/native-desktop.json` 只列启用的业务根，包含官方 publication/ownership 文件路径、profile、Node 路径与指定 model/provider，无令牌。部署脚本只给本次项目启用原生传输。

真实验收必须另行通过 Gateway 提交有唯一 request_key 的任务，绑定原聊天队列，独立核对原生项目、持久会话 cwd、实际命令及成果，再记录 accepted。模拟测试、空白会话预检不替代此验收。
