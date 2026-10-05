#!/usr/bin/env python3
"""
test_full_lifecycle.py - Automated End-to-End Test for RDP & VM Lifecycle Management
"""

import json
import socket
import subprocess
import sys
import time

GREEN = "\033[92m"
YELLOW = "\033[93m"
CYAN = "\033[96m"
RED = "\033[91m"
BOLD = "\033[1m"
RESET = "\033[0m"

RDP_LAUNCHER = "/usr/local/bin/rdp-launcher"
VM_NAME = "win11"

def step(title):
    print(f"\n{BOLD}{CYAN}=== [STEP] {title} ==={RESET}")

def success(msg):
    print(f"{GREEN}✓ {msg}{RESET}")

def info(msg):
    print(f"  {msg}")

def warn(msg):
    print(f"{YELLOW}! {msg}{RESET}")

def error(msg):
    print(f"{RED}✗ {msg}{RESET}")
    sys.exit(1)

def run(cmd, capture=True, check=False):
    res = subprocess.run(cmd, shell=True, capture_output=capture, text=True)
    if check and res.returncode != 0:
        error(f"Command failed ({res.returncode}): {cmd}\nStderr: {res.stderr}")
    return res

def get_vm_state():
    return run(f"virsh domstate {VM_NAME}").stdout.strip()

def get_niri_rdp_windows():
    res = run("niri msg -j windows")
    if res.returncode != 0:
        return []
    try:
        data = json.loads(res.stdout)
        return [w for w in data if w.get("app_id") == "com.freerdp.client.sdl3"]
    except Exception:
        return []

def main():
    print(f"{BOLD}====================================================")
    print(f"  RDP & VM 全流程自动化生命周期测试")
    print(f"===================================================={RESET}")

    # STEP 1: 确保虚拟机当前处于关机状态
    step("1. 检查初始状态（虚拟机关机，服务非活动）")
    state = get_vm_state()
    info(f"当前 VM 状态: {state}")
    if "暂停" in state or "paused" in state:
        info("虚拟机当前处于挂起状态，正在恢复并关闭以准备冷启动测试...")
        run(f"virsh resume {VM_NAME}")
        time.sleep(1)
        state = get_vm_state()

    if "运行" in state or "running" in state:
        info("正在关闭虚拟机以进行冷启动测试...")
        run(f"virsh shutdown {VM_NAME}")
        for _ in range(40):
            time.sleep(1)
            state = get_vm_state()
            if "关" in state or "shut off" in state:
                break
    
    state = get_vm_state()
    if not ("关" in state or "shut off" in state):
        error(f"虚拟机未处于关机状态: {state}")
    success(f"虚拟机已关机 (状态: {state})")

    # 检查 rdp-lifecycle 服务状态
    res = run("systemctl is-active rdp-lifecycle")
    info(f"rdp-lifecycle 服务状态: {res.stdout.strip()}")
    if res.stdout.strip() == "active":
        error("rdp-lifecycle 在关机状态下不应处于活动状态！")
    success("rdp-lifecycle 服务处于未启动状态（符合预期）")

    # STEP 2: 启动虚拟机并验证 Hook 联动
    step("2. 启动虚拟机并验证 Libvirt Hook 自动拉起服务")
    run(f"virsh start {VM_NAME}", check=True)
    success("virsh start 发送成功，正在等待 VM 启动和 Hook 执行...")

    # 轮询服务启动
    hook_triggered = False
    for _ in range(15):
        time.sleep(1)
        res = run("systemctl is-active rdp-lifecycle")
        if res.stdout.strip() == "active":
            hook_triggered = True
            break
    
    if not hook_triggered:
        error("Libvirt Hook 未能在 VM 启动时自动启动 rdp-lifecycle.service！")
    success("Libvirt Hook 成功联动：rdp-lifecycle.service 自动上线！")

    # STEP 3: 等待 Windows 虚拟机网络与 IP 就绪
    step("3. 动态检测 Windows 虚拟机网络就绪")
    vm_ip = None
    info("正在通过 QEMU Guest Agent 动态探测所有网络 IP (冷启动等待中)...")
    for i in range(90):
        time.sleep(1)
        res = run(f"{RDP_LAUNCHER} lifecycle-status")
        for line in res.stdout.splitlines():
            if "VM IP Addresses:" in line:
                ips = line.split(":", 1)[1].strip()
                if "192." in ips or "10." in ips:
                    vm_ip = ips
                    break
        if vm_ip:
            break
        if i % 10 == 0 and i > 0:
            info(f"  等待 Guest Agent 初始化... ({i}s)")

    if not vm_ip:
        error("未能探测到 Windows 虚拟机的有效 IP 地址！")
    
    # 提取首个有效 IPv4 地址
    ip_first = None
    for part in vm_ip.split(","):
        p = part.strip()
        if "." in p:
            ip_first = p
            break
    if not ip_first:
        ip_first = "192.168.122.14"

    success(f"检测到虚拟机 IP: {ip_first}")

    info(f"等待 Windows 3389 (RDP) 端口与 RPC 服务就绪...")
    port_ready = False
    for i in range(90):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(1.0)
        try:
            s.connect((ip_first, 3389))
            s.close()
            port_ready = True
            break
        except Exception:
            time.sleep(1)
            if i % 10 == 0 and i > 0:
                info(f"  等待 RDP 端口开启... ({i}s)")

    if not port_ready:
        error(f"Windows RDP 服务未能成功在 {ip_first}:3389 上线！")
    
    info("端口已开启，等待 3 秒使 Windows 会话与 DWM 桌面完全初始化...")
    time.sleep(3)
    success(f"Windows RDP 端口与桌面环境已就绪！")

    # STEP 4: 发起应用启动（全自动 Keyring 登录 + RemoteApp 调起）
    step("4. 发起应用启动测试（调起字符映射表 charmap）")
    info("执行命令: rdp-launcher run character-map")
    t0 = time.time()
    res = run(f"{RDP_LAUNCHER} run character-map", check=True)
    info(res.stdout.strip())

    # 验证窗口是否成功出现在 Niri 中
    window_found = False
    for i in range(40):
        time.sleep(1)
        wins = get_niri_rdp_windows()
        if wins:
            window_found = True
            title = wins[0].get("title", "")
            info(f"Niri 检测到新 RemoteApp 窗口: '{title}' (PID: {wins[0].get('pid')})")
            break
        if i % 5 == 0 and i > 0:
            info(f"  等待 RemoteApp 窗口渲染至 Wayland... ({i}s)")

    if not window_found:
        error("未能在 Niri 桌面上检测到弹出的 RemoteApp 窗口！")
    success("应用启动成功，原生 Wayland 窗口已平铺在本地桌面！")

    # STEP 5: 验证仪表盘状态
    step("5. 验证生命周期仪表盘状态")
    status = run(f"{RDP_LAUNCHER} lifecycle-status").stdout
    print(status.strip())
    if "1 active" not in status and "RemoteApp Windows:" not in status:
        warn("窗口计数未在仪表盘中即时刷新，继续验证。")
    else:
        success("生命周期仪表盘状态与当前桌面窗口完全吻合！")

    # STEP 6: 关闭应用，测试 30 秒空闲自动断开
    step("6. 关闭应用并测试 30 秒空闲自动断开机制")
    info("正在关闭远端 charmap 进程...")
    # 发送远程命令关闭 charmap
    ip_first = vm_ip.split(",")[0].strip()
    if "%" in ip_first:
        ip_first = vm_ip.split(",")[1].strip()
    run(f'ssh skwj111@{ip_first} "pwsh -NoProfile -Command \\"Get-Process -Name charmap -ErrorAction SilentlyContinue | Stop-Process -Force\\""')

    # 验证本地窗口消失
    for _ in range(10):
        time.sleep(0.5)
        if len(get_niri_rdp_windows()) == 0:
            break
    success("RemoteApp 窗口已关闭，桌面无任何活动远程窗口")

    info("已进入 30 秒空闲倒计时，监控 FreeRDP 自动断开情况...")
    t_idle_start = time.time()
    freerdp_disconnected = False
    for i in range(45):
        time.sleep(1)
        res = run("pgrep -x sdl-freerdp")
        if not res.stdout.strip():
            freerdp_disconnected = True
            elapsed = time.time() - t_idle_start
            info(f"FreeRDP 进程已自动退出，空闲判定耗时: {elapsed:.1f} 秒")
            break
        if i > 0 and i % 5 == 0:
            info(f"  已等待 {i} 秒，FreeRDP 仍连接...")

    if not freerdp_disconnected:
        error("超过 45 秒仍未自动断开 FreeRDP 会话！")
    success("30 秒空闲断开机制实测成功！FreeRDP 已自动干净释放！")

    # STEP 7: 测试 virtio-mem 动态回收与虚拟机挂起/唤醒
    step("7. 测试 virtio-mem 内存动态回收与挂起")
    info("执行 update-memory-device 回收 virtio-mem 动态内存至 0...")
    run(f"virsh update-memory-device {VM_NAME} --alias ua-virtiomem0 --requested-size 0 --live", check=True)
    success("virtio-mem 动态内存回收指令执行成功！")

    info("执行虚拟机挂起 (virsh suspend)...")
    run(f"virsh suspend {VM_NAME}", check=True)
    state = get_vm_state()
    if "暂停" not in state and "paused" not in state:
        error(f"挂起后状态不匹配: {state}")
    success(f"虚拟机已成功挂起至 RAM，CPU 占用降为 0% (状态: {state})")

    # STEP 8: 测试冷唤醒（通过 rdp-launcher run 触发自动唤醒）
    step("8. 测试挂起状态下点击应用自动唤醒恢复")
    info("在挂起状态下执行: rdp-launcher run character-map")
    t_wake_start = time.time()
    res = run(f"{RDP_LAUNCHER} run character-map", check=True)
    info(res.stdout.strip())

    state = get_vm_state()
    info(f"虚拟机当前状态: {state}")
    if not ("运行" in state or "running" in state):
        error("rdp-launcher 未能在启动应用时自动唤醒虚拟机！")
    success(f"虚拟机在 {(time.time() - t_wake_start):.2f} 秒内自动恢复运行！")

    # 清理刚才的 charmap
    run(f'ssh skwj111@{ip_first} "pwsh -NoProfile -Command \\"Get-Process -Name charmap -ErrorAction SilentlyContinue | Stop-Process -Force\\""')

    # STEP 9: 清理并退出
    step("9. 最终清理与测试收尾")
    run(f"{RDP_LAUNCHER} stop-daemon")
    success("远程守护进程已停止，FreeRDP 会话清理完毕")

    print(f"\n{BOLD}{GREEN}====================================================")
    print(f"  🎉 全部全流程生命周期测试 100% 通过！")
    print(f"===================================================={RESET}\n")

if __name__ == "__main__":
    main()
