import { useEffect, useRef, useState } from "react";
import type { AppExtensionPage } from "@agent-hud/app/extension";
import { Button } from "@agent-hud/ui/components/button";
import { Input } from "@agent-hud/ui/components/input";
import "./workbench.css";

type Request = (method: string, params?: unknown) => Promise<unknown>;
type Task = {
  id: string;
  executor: string;
  objective?: string;
  status?: string;
  review?: string;
  result?: string;
};
type WorkbenchState = {
  connected?: boolean;
  projectRoot?: string;
  workbenchRoot?: string;
  error?: string;
  tasks?: Task[];
  executors?: { id: string; name: string; state: string }[];
  dispatchExecutors?: { id: string; name: string }[];
};

const statuses: Record<string, string> = {
  queued: "排队中",
  running: "执行中",
  waiting_approval: "等待桌面审批",
  uncertain: "结果待确认",
  completed: "已交付",
  failed: "失败",
  cancelled: "已取消",
  blocked: "受阻",
};

function WorkbenchPage({ request }: { request: Request }) {
  const currentRequest = useRef(request);
  currentRequest.current = request;
  const [state, setState] = useState<WorkbenchState>({});
  const [projectRoot, setProjectRoot] = useState("Z:\\Agent HUD");
  const [workbenchRoot, setWorkbenchRoot] = useState("Z:\\Hermes\\workbench");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [executor, setExecutor] = useState("");
  const [objective, setObjective] = useState("");
  const [boundary, setBoundary] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [key, setKey] = useState(() => `hud-${crypto.randomUUID()}`);
  const [attempted, setAttempted] = useState(false);
  const [selected, setSelected] = useState<Task | null>(null);

  useEffect(() => {
    let active = true;
    let pending = false;
    let initial = true;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const value = (await currentRequest.current(
          "workbench_status",
        )) as WorkbenchState;
        if (active) {
          setState(value);
          setExecutor(
            (previous) => previous || value.dispatchExecutors?.[0]?.id || "",
          );
          if (initial) {
            if (value.projectRoot) setProjectRoot(value.projectRoot);
            if (value.workbenchRoot) setWorkbenchRoot(value.workbenchRoot);
            initial = false;
          }
        }
      } catch (error) {
        if (active) setError(String(error));
      } finally {
        pending = false;
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, []);

  const act = async (method: string, params?: unknown) => {
    setBusy(true);
    setError("");
    try {
      return await request(method, params);
    } catch (error) {
      setError(String(error));
      return null;
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="workbench-page">
      <h2>协作工作台</h2>
      <p>
        连接已有的大肥鱼 Gateway 和 Antigravity
        桌面工作台。只显示所选项目的任务。
      </p>
      <section>
        <label>
          所属项目根目录
          <Input
            value={projectRoot}
            onChange={(event) => setProjectRoot(event.target.value)}
            disabled={busy || state.connected}
          />
        </label>
        <label>
          工作台安装目录
          <Input
            value={workbenchRoot}
            onChange={(event) => setWorkbenchRoot(event.target.value)}
            disabled={busy || state.connected}
          />
        </label>
        <div className="workbench-actions">
          <Button
            disabled={busy}
            onClick={async () => {
              const value = await act("workbench_connect", {
                projectRoot,
                workbenchRoot,
              });
              if (value) {
                const connected = value as WorkbenchState;
                setState(connected);
                setExecutor(
                  (previous) =>
                    previous || connected.dispatchExecutors?.[0]?.id || "",
                );
                setSelected(null);
              }
            }}
          >
            {state.connected ? "刷新连接" : "连接项目"}
          </Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={async () => {
              const value = await act("workbench_disconnect");
              if (value) {
                setState(value as WorkbenchState);
                setSelected(null);
              }
            }}
          >
            断开
          </Button>
          <span role="status">
            {state.connected ? "已连接 · 后台观察任务状态" : "未连接"}
          </span>
        </div>
        {state.executors?.map((item) => (
          <p key={item.id}>
            <strong>{item.name}</strong> · {item.state}
          </p>
        ))}
      </section>
      {(error || state.error) && (
        <p role="alert" className="workbench-error">
          {error || state.error}
        </p>
      )}
      <section>
        <h3>手动派工</h3>
        <p>
          点击后会真实执行。Antigravity 使用 Gemini 4 Argon
          High；成果写入本项目的协作办公区。这里的派工需另行交给 Codex
          独立验收。
        </p>
        <label>
          执行端
          <select
            value={executor}
            onChange={(event) => setExecutor(event.target.value)}
            disabled={busy || attempted}
          >
            <option value="" disabled>
              选择执行端
            </option>
            {state.dispatchExecutors?.map((item) => (
              <option value={item.id} key={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          任务目标
          <textarea
            rows={3}
            value={objective}
            onChange={(event) => setObjective(event.target.value)}
            disabled={busy || attempted}
          />
        </label>
        <label>
          允许修改的范围与边界
          <textarea
            rows={2}
            value={boundary}
            onChange={(event) => setBoundary(event.target.value)}
            disabled={busy || attempted}
          />
        </label>
        <label>
          验收标准
          <textarea
            rows={2}
            value={acceptance}
            onChange={(event) => setAcceptance(event.target.value)}
            disabled={busy || attempted}
          />
        </label>
        <small>本次提交标识：{key}</small>
        <div className="workbench-actions">
          <Button
            disabled={
              busy ||
              !state.connected ||
              !executor ||
              attempted ||
              !objective.trim() ||
              !boundary.trim() ||
              !acceptance.trim()
            }
            onClick={async () => {
              setAttempted(true);
              const value = await act("workbench_submit", {
                executor,
                objective,
                boundary,
                acceptance,
                request_key: key,
              });
              if (value) setSelected({ ...(value as Task), executor });
            }}
          >
            发送任务
          </Button>
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              setKey(`hud-${crypto.randomUUID()}`);
              setAttempted(false);
              setObjective("");
              setBoundary("");
              setAcceptance("");
              setSelected(null);
              setError("");
            }}
          >
            填写新任务
          </Button>
        </div>
        {attempted && (
          <p>
            已尝试提交。请先查看下方任务；连接中断时结果可能待确认，勿填写相同的新任务。
          </p>
        )}
      </section>
      <section>
        <h3>项目任务</h3>
        <p>
          “已交付”表示执行端完成；“验收通过”须有独立核验记录。已有历史任务首次连接时不播放动画。
        </p>
        <div className="workbench-tasks">
          {state.tasks?.map((task) => (
            <button
              key={`${task.executor}:${task.id}`}
              type="button"
              onClick={async () => {
                const value = await act("workbench_task", {
                  executor: task.executor,
                  task_id: task.id,
                });
                if (value)
                  setSelected({ ...(value as Task), executor: task.executor });
              }}
              disabled={busy}
            >
              <strong>
                {state.dispatchExecutors?.find(
                  (item) => item.id === task.executor,
                )?.name ?? task.executor}{" "}
                · {statuses[task.status ?? ""] ?? task.status}
              </strong>
              <span>{task.objective}</span>
              <small>
                {task.id} ·{" "}
                {task.review === "accepted"
                  ? "验收通过"
                  : task.review === "rejected"
                    ? "需返工"
                    : "待独立验收"}
              </small>
            </button>
          ))}
          {!state.tasks?.length && <p>当前项目暂无任务。</p>}
        </div>
        {selected && (
          <article className="workbench-result">
            <h3>{selected.objective || selected.id}</h3>
            <p>{statuses[selected.status ?? ""] ?? selected.status}</p>
            <pre>{selected.result || "尚无交付结果"}</pre>
          </article>
        )}
      </section>
    </div>
  );
}

export const workbenchPage: AppExtensionPage = {
  id: "workbench",
  title: "Collaboration workbench",
  description:
    "Project tasks from the existing local Hermes and Antigravity workbench.",
  zh: ["协作工作台", "连接大肥鱼与 Antigravity，按项目查看任务、派工和交付。"],
  render: ({ request }) => <WorkbenchPage request={request} />,
};
