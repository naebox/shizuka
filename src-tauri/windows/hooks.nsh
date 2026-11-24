; Notify Windows Explorer to flush its Shell Icon Cache immediately after
; installing, upgrading, or uninstalling Shizuka so Taskbar & Start Menu
; icons update without requiring an Explorer restart.
; SHCNE_ASSOCCHANGED = 0x08000000, SHCNF_IDLIST | SHCNF_FLUSHNOWAIT = 0x2000
!macro NSIS_HOOK_POSTINSTALL
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0x2000, p 0, p 0)'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0x2000, p 0, p 0)'
!macroend
