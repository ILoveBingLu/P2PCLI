; Tauri 的安装脚本在插入界面页面之前引入这个文件，这里放界面上的调整。
; 图片由 scripts/render-art.mjs 从同目录的 SVG 生成。

; 每页顶栏的图标放在右边，标题和说明留在左边
!define MUI_HEADERIMAGE_RIGHT

; 关掉窗口后 psmux 仍占着安装目录里的 psmux.exe。不先放开这个文件，更新和覆盖安装会停在“无法写入”。
; 先结束进程；结束不掉就把旧文件改名，新文件照样能写进去。正在跑的旧进程继续用改名后的文件，直到它退出。
!macro P2PCLI_RELEASE_PSMUX
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /F /IM psmux.exe /T'
  Pop $0
  Sleep 500
  IfFileExists "$INSTDIR\psmux.exe" 0 p2pcli_psmux_ready
    ClearErrors
    Rename "$INSTDIR\psmux.exe" "$INSTDIR\psmux.exe.old"
  p2pcli_psmux_ready:
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro P2PCLI_RELEASE_PSMUX
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro P2PCLI_RELEASE_PSMUX
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Delete "$INSTDIR\psmux.exe.old"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$INSTDIR\psmux.exe.old"
!macroend
