/**
 * 手机端（Android、iOS）：只当连接的一方，界面换成手机的样式，桌面才有的功能（本机终端、被别人连、
 * 代码编辑器窗口、端口转发、托盘、自动更新、多机互动）不显示。
 * 桌面浏览器里预览时在地址后加 ?mobile 也按手机显示。
 */
export const isMobile =
  /Android|iPhone|iPad|iPod/i.test(navigator.userAgent) ||
  new URLSearchParams(location.search).has("mobile");
