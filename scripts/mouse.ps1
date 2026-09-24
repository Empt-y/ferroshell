# Synthetic mouse input for manual/scripted UI checks (physical pixel coordinates).
# Usage: scripts\mouse.ps1 -X 118 -Y 992 [-Action move|left|right|middle] [-Wait 0]
param([int]$X, [int]$Y, [string]$Action = "move", [int]$Wait = 0)
Add-Type -Namespace W -Name M -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, System.UIntPtr e);
'@
[W.M]::SetProcessDPIAware() | Out-Null
[W.M]::SetCursorPos($X, $Y) | Out-Null
Start-Sleep -Milliseconds 60
switch ($Action) {
    "left"   { [W.M]::mouse_event(0x02, 0, 0, 0, [UIntPtr]::Zero); [W.M]::mouse_event(0x04, 0, 0, 0, [UIntPtr]::Zero) }
    "right"  { [W.M]::mouse_event(0x08, 0, 0, 0, [UIntPtr]::Zero); [W.M]::mouse_event(0x10, 0, 0, 0, [UIntPtr]::Zero) }
    "middle" { [W.M]::mouse_event(0x20, 0, 0, 0, [UIntPtr]::Zero); [W.M]::mouse_event(0x40, 0, 0, 0, [UIntPtr]::Zero) }
}
if ($Wait -gt 0) { Start-Sleep -Milliseconds $Wait }
