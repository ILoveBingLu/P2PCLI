import AVFoundation
import CoreMedia
import Observation
import SwiftUI
import UIKit
import VideoToolbox

/// 触屏怎么当鼠标用
enum PointerMode {
    /// 触控板：手指滑动挪指针，点一下点在指针上。默认：手指粗，小按钮也点得准
    case trackpad
    /// 直接点：手指点哪儿就点哪儿
    case touch
}

/// 手机上正在看的一路远程画面。
///
/// 对方电脑硬件编码的 H.264/H.265 从 Rust 原样送来，交给系统的显示层（AVSampleBufferDisplayLayer）硬件解码、直接显示；
/// 手指的操作在 [ScreenCanvas] 里换成鼠标键盘事件发回去。
@MainActor
@Observable
final class RemoteScreen: Identifiable {
    enum Phase: Equatable {
        /// 等对方同意
        case opening
        case live
        /// 对方暂停发画面：relay 为真是连接走了中继
        case paused(relay: Bool)
        case closed(String?)
    }

    struct Stats: Equatable {
        var bitrate: UInt32
        var fps: UInt32
    }

    let id = UUID()
    let endpointId: String
    let connId: UInt64
    let name: String
    /// 对方的系统：Mac 上的 ⌘ 在 Windows 上是 Win 键
    let platform: String
    var phase: Phase = .opening
    var started: ScreenStartedView?
    var display = ""
    var quality: ScreenQuality = .balanced
    var mode: PointerMode = .trackpad
    var stats: Stats?
    /// 粘住的修饰键（ControlLeft、AltLeft、MetaLeft、ShiftLeft）：下一次按键、点击带上它们，之后松开
    var modifiers: [String] = []
    var keyboardUp = false
    /// 已经出过画面
    var hasVideo = false
    /// 收起工具栏，只留一个小圆钮
    var toolbarCollapsed = false

    @ObservationIgnored let canvas = ScreenCanvas()
    @ObservationIgnored private var handle: ScreenView?
    @ObservationIgnored private var bridge: ScreenBridge?
    @ObservationIgnored private let opener: (ScreenQuality, [ScreenCodec], ScreenListener) async throws -> ScreenView

    init(endpointId: String, connId: UInt64, name: String, platform: String,
         opener: @escaping (ScreenQuality, [ScreenCodec], ScreenListener) async throws -> ScreenView) {
        self.endpointId = endpointId
        self.connId = connId
        self.name = name
        self.platform = platform
        self.opener = opener
        canvas.screen = self
    }

    var isMac: Bool { platform.lowercased().contains("mac") || platform.lowercased() == "darwin" }
    var canControl: Bool { started?.control ?? false }

    func open() {
        phase = .opening
        hasVideo = false
        let bridge = ScreenBridge(owner: self, decoder: canvas.decoder)
        self.bridge = bridge
        canvas.decoder.onNeedKeyframe = { [weak self] in
            DispatchQueue.main.async { self?.handle?.requestKeyframe() }
        }
        var codecs: [ScreenCodec] = [.h264]
        if VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC) { codecs.insert(.h265, at: 0) }
        let quality = self.quality
        Task {
            do {
                let h = try await opener(quality, codecs, bridge)
                // 等的时候已经关掉了
                guard case .opening = phase else { h.stop(); return }
                handle = h
                let s = h.started()
                started = s
                display = s.display
                phase = .live
                UIApplication.shared.isIdleTimerDisabled = true
            } catch {
                let text = errorText(error)
                phase = .closed(text.hasPrefix("relay:") ? t("现在这条连接走的是中继，远程画面只走直连。终端照常能用；直连接通后再打开画面") : text)
            }
        }
    }

    func stop() {
        handle?.stop()
        handle = nil
        bridge = nil
        if case .closed = phase {} else { phase = .closed(nil) }
        UIApplication.shared.isIdleTimerDisabled = false
    }

    /// 回到前台：显示层在后台会失效，清掉重来，要一个关键帧
    func resume() {
        canvas.decoder.reset()
    }

    func switchDisplay(_ id: String) {
        handle?.switchDisplay(display: id)
    }

    func setQuality(_ q: ScreenQuality) {
        quality = q
        handle?.setQuality(quality: q)
    }

    // MARK: 输入

    func send(_ input: ScreenInput) {
        guard canControl else { return }
        handle?.send(input: input)
    }

    /// 点一下：带上粘住的修饰键，点完松开
    func click(_ button: ScreenMouseButton, at p: CGPoint) {
        guard canControl else { return }
        let mods = takeModifiers()
        for m in mods { send(.key(code: m, down: true)) }
        send(.move(x: p.x, y: p.y))
        send(.button(button: button, down: true, x: p.x, y: p.y))
        send(.button(button: button, down: false, x: p.x, y: p.y))
        for m in mods.reversed() { send(.key(code: m, down: false)) }
    }

    /// 拖动开始：修饰键按住到拖完
    @ObservationIgnored private var dragMods: [String] = []

    func dragBegan(at p: CGPoint) {
        guard canControl else { return }
        dragMods = takeModifiers()
        for m in dragMods { send(.key(code: m, down: true)) }
        send(.move(x: p.x, y: p.y))
        send(.button(button: .left, down: true, x: p.x, y: p.y))
    }

    func dragEnded(at p: CGPoint) {
        send(.button(button: .left, down: false, x: p.x, y: p.y))
        for m in dragMods.reversed() { send(.key(code: m, down: false)) }
        dragMods = []
    }

    func typeText(_ text: String) {
        guard canControl, !text.isEmpty else { return }
        handle?.typeText(text: text, modifiers: takeModifiers())
    }

    func tapKey(_ code: String) {
        guard canControl else { return }
        handle?.tapKey(code: code, modifiers: takeModifiers())
    }

    func toggleModifier(_ code: String) {
        if let i = modifiers.firstIndex(of: code) { modifiers.remove(at: i) } else { modifiers.append(code) }
    }

    private func takeModifiers() -> [String] {
        let m = modifiers
        modifiers = []
        return m
    }

    // MARK: Rust 的回调（已经切到主线程）

    fileprivate func onCursor(x: Double, y: Double, visible: Bool) { canvas.remoteCursor(x: x, y: y, visible: visible) }
    fileprivate func onCursorShape(_ s: ScreenCursorShape) { canvas.setCursorShape(s) }
    fileprivate func onConfig(_ c: ScreenVideoConfig) { canvas.setVideoSize(CGSize(width: Int(c.width), height: Int(c.height))) }
    fileprivate func onFirstFrame() { hasVideo = true }
    fileprivate func onPaused(relay: Bool) { if case .live = phase { phase = .paused(relay: relay) } }
    fileprivate func onResumed() { if case .paused = phase { phase = .live } }
    fileprivate func onSwitched(_ d: String) { display = d }
    fileprivate func onStats(bitrate: UInt32, fps: UInt32) { stats = Stats(bitrate: bitrate, fps: fps) }
    fileprivate func onClosed(_ reason: String?) {
        handle = nil
        if case .closed = phase { return }
        phase = .closed(reason.map(tr))
        UIApplication.shared.isIdleTimerDisabled = false
    }
}

/// Rust 那边的回调：画面直接交给解码器（在它自己的队列上），其他的切回主线程
private final class ScreenBridge: ScreenListener, @unchecked Sendable {
    weak var owner: RemoteScreen?
    let decoder: VideoDecoder
    private var sawFrame = false

    init(owner: RemoteScreen, decoder: VideoDecoder) {
        self.owner = owner
        self.decoder = decoder
    }

    private func main(_ f: @escaping @MainActor (RemoteScreen) -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let o = self.owner { f(o) }
            }
        }
    }

    func onConfig(config: ScreenVideoConfig) {
        decoder.configure(config)
        main { $0.onConfig(config) }
    }

    func onFrame(key: Bool, data: Data) {
        decoder.decode(key: key, data: data)
        if !sawFrame {
            sawFrame = true
            main { $0.onFirstFrame() }
        }
    }

    func onCursor(x: Double, y: Double, visible: Bool) { main { $0.onCursor(x: x, y: y, visible: visible) } }
    func onCursorShape(shape: ScreenCursorShape) { main { $0.onCursorShape(shape) } }
    func onPaused(relay: Bool) { main { $0.onPaused(relay: relay) } }
    func onResumed() { main { $0.onResumed() } }
    func onSwitched(display: String) { main { $0.onSwitched(display) } }
    func onStats(bitrate: UInt32, fps: UInt32) { main { $0.onStats(bitrate: bitrate, fps: fps) } }
    func onClosed(reason: String?) { main { $0.onClosed(reason) } }
}

// MARK: - 解码显示

/// 显示层：系统硬件解码，解出来直接上屏，不经过我们的代码
final class VideoLayerView: UIView {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }
    var videoLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }
}

/// 把 AVCC 帧包成 CMSampleBuffer 交给显示层。都在自己的串行队列上做，不占界面线程
final class VideoDecoder: @unchecked Sendable {
    let view = VideoLayerView()
    private let queue = DispatchQueue(label: "p2pcli.screen.decode", qos: .userInteractive)
    private var format: CMVideoFormatDescription?
    /// 下一帧要从关键帧开始
    private var needKey = true
    private let renderer: AVSampleBufferVideoRenderer
    /// 解码出错时要一个关键帧（主线程以外调）
    var onNeedKeyframe: (() -> Void)?

    init() {
        view.videoLayer.videoGravity = .resize
        view.backgroundColor = .black
        renderer = view.videoLayer.sampleBufferRenderer
    }

    func configure(_ c: ScreenVideoConfig) {
        queue.async { [self] in
            let atom = c.codec == .h265 ? "hvcC" : "avcC"
            let ext: [CFString: Any] = [
                kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: [atom: c.description as CFData],
            ]
            var fd: CMVideoFormatDescription?
            let status = CMVideoFormatDescriptionCreate(
                allocator: kCFAllocatorDefault,
                codecType: c.codec == .h265 ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264,
                width: Int32(c.width), height: Int32(c.height),
                extensions: ext as CFDictionary, formatDescriptionOut: &fd)
            guard status == noErr, let fd else { return }
            format = fd
            needKey = true
        }
    }

    func decode(key: Bool, data: Data) {
        queue.async { [self] in
            guard let format else { return }
            if needKey && !key { return }
            if renderer.status == .failed || renderer.requiresFlushToResumeDecoding {
                renderer.flush()
                needKey = true
                onNeedKeyframe?()
                if !key { return }
            }
            guard let sample = Self.sample(data, format: format, key: key) else { return }
            renderer.enqueue(sample)
            needKey = false
        }
    }

    /// 切回前台、显示层失效以后：清掉重来
    func reset() {
        queue.async { [self] in
            renderer.flush()
            needKey = true
        }
        onNeedKeyframe?()
    }

    private static func sample(_ data: Data, format: CMVideoFormatDescription, key: Bool) -> CMSampleBuffer? {
        let len = data.count
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault, memoryBlock: nil, blockLength: len, blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil, offsetToData: 0, dataLength: len, flags: kCMBlockBufferAssureMemoryNowFlag,
            blockBufferOut: &block) == noErr, let block else { return nil }
        let copied = data.withUnsafeBytes { raw -> OSStatus in
            guard let base = raw.baseAddress else { return -1 }
            return CMBlockBufferReplaceDataBytes(with: base, blockBuffer: block, offsetIntoDestination: 0, dataLength: len)
        }
        guard copied == noErr else { return nil }
        var sample: CMSampleBuffer?
        var size = len
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format, sampleCount: 1,
            sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1, sampleSizeArray: &size,
            sampleBufferOut: &sample) == noErr, let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true), CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            // 一收到就显示，不按时间戳排
            CFDictionarySetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
            if !key {
                CFDictionarySetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_NotSync).toOpaque(),
                                     Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
            }
        }
        return sample
    }
}

// MARK: - 画面和手势

/// 画面、指针和手势。坐标都换成相对对方屏幕的 0~1 再发
final class ScreenCanvas: UIView, UIGestureRecognizerDelegate {
    weak var screen: RemoteScreen?
    let decoder = VideoDecoder()
    private let cursorView = UIImageView()
    let keyCatcher = KeyCatcher()

    /// 画面的像素尺寸（解码配置里的）
    private var videoSize = CGSize(width: 16, height: 10)
    /// 放大倍数和平移（相对居中摆放）
    private var zoom: CGFloat = 1
    private var pan = CGPoint.zero
    /// 指针在对方屏幕上的位置（0~1）
    private var cursor = CGPoint(x: 0.5, y: 0.5)
    private var cursorVisible = true
    private var cursorShape: ScreenCursorShape?
    /// 最近一次本机挪指针的时间：这之后一小会儿不信对方报回来的位置，免得指针往回跳
    private var lastLocalMove = Date.distantPast

    private lazy var oneFingerPan = UIPanGestureRecognizer(target: self, action: #selector(onOneFingerPan(_:)))
    private lazy var twoFingerPan = UIPanGestureRecognizer(target: self, action: #selector(onTwoFingerPan(_:)))
    private lazy var pinch = UIPinchGestureRecognizer(target: self, action: #selector(onPinch(_:)))
    private lazy var tap = UITapGestureRecognizer(target: self, action: #selector(onTap(_:)))
    private lazy var twoFingerTap = UITapGestureRecognizer(target: self, action: #selector(onTwoFingerTap(_:)))
    private lazy var longPress = UILongPressGestureRecognizer(target: self, action: #selector(onLongPress(_:)))

    init() {
        super.init(frame: .zero)
        backgroundColor = .black
        clipsToBounds = true
        isMultipleTouchEnabled = true
        addSubview(decoder.view)
        cursorView.isHidden = true
        cursorView.isUserInteractionEnabled = false
        addSubview(cursorView)
        keyCatcher.canvas = self
        addSubview(keyCatcher)

        oneFingerPan.maximumNumberOfTouches = 1
        twoFingerPan.minimumNumberOfTouches = 2
        twoFingerPan.maximumNumberOfTouches = 2
        twoFingerTap.numberOfTouchesRequired = 2
        longPress.minimumPressDuration = 0.35
        for g in [oneFingerPan, twoFingerPan, pinch, tap, twoFingerTap, longPress] as [UIGestureRecognizer] {
            g.delegate = self
            addGestureRecognizer(g)
        }
        tap.require(toFail: twoFingerTap)
    }

    required init?(coder: NSCoder) { fatalError() }

    private var mode: PointerMode { screen?.mode ?? .trackpad }

    // MARK: 摆放

    /// 画面按比例放进来，再按放大倍数和平移挪
    private var contentRect: CGRect {
        let b = bounds.size
        guard b.width > 0, b.height > 0, videoSize.width > 0, videoSize.height > 0 else { return .zero }
        let s = min(b.width / videoSize.width, b.height / videoSize.height) * zoom
        let w = videoSize.width * s, h = videoSize.height * s
        func axis(_ size: CGFloat, _ bound: CGFloat, _ offset: CGFloat) -> CGFloat {
            if size <= bound { return (bound - size) / 2 }
            return min(0, max(bound - size, (bound - size) / 2 + offset))
        }
        return CGRect(x: axis(w, b.width, pan.x), y: axis(h, b.height, pan.y), width: w, height: h)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        layoutContent()
    }

    private func layoutContent() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        decoder.view.frame = contentRect
        layoutCursor()
        CATransaction.commit()
    }

    /// 平移量收在画面能盖住的范围里（缩小到比屏幕小时居中）
    private func clampPan() {
        let r = contentRect
        let b = bounds.size
        pan.x = r.width <= b.width ? 0 : r.minX - (b.width - r.width) / 2
        pan.y = r.height <= b.height ? 0 : r.minY - (b.height - r.height) / 2
    }

    func setVideoSize(_ size: CGSize) {
        videoSize = size
        clampPan()
        layoutContent()
    }

    private func normalized(_ p: CGPoint) -> CGPoint {
        let r = contentRect
        guard r.width > 0, r.height > 0 else { return CGPoint(x: 0.5, y: 0.5) }
        return CGPoint(x: min(1, max(0, (p.x - r.minX) / r.width)), y: min(1, max(0, (p.y - r.minY) / r.height)))
    }

    private func viewPoint(_ n: CGPoint) -> CGPoint {
        let r = contentRect
        return CGPoint(x: r.minX + n.x * r.width, y: r.minY + n.y * r.height)
    }

    // MARK: 指针

    func setCursorShape(_ s: ScreenCursorShape) {
        cursorShape = s
        let w = Int(s.width), h = Int(s.height)
        guard w > 0, h > 0, s.rgba.count >= w * h * 4,
              let provider = CGDataProvider(data: s.rgba as CFData),
              let image = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                                  provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        else { return }
        cursorView.image = UIImage(cgImage: image)
        layoutCursor()
    }

    func remoteCursor(x: Double, y: Double, visible: Bool) {
        cursorVisible = visible
        if Date().timeIntervalSince(lastLocalMove) > 0.4 {
            cursor = CGPoint(x: x, y: y)
        }
        layoutCursor()
    }

    /// 指针图案按对方屏幕上的大小缩放，太小了手机上看不清，至少 20 点高
    private func layoutCursor() {
        guard let s = cursorShape, cursorView.image != nil else {
            cursorView.isHidden = true
            return
        }
        cursorView.isHidden = !cursorVisible
        let r = contentRect
        let screenWidth = CGFloat(screen?.started?.displays.first { $0.id == screen?.display }?.width ?? UInt32(videoSize.width))
        var scale = screenWidth > 0 ? r.width / screenWidth : 1
        scale = max(scale, 20 / max(1, CGFloat(s.height)))
        let size = CGSize(width: CGFloat(s.width) * scale, height: CGFloat(s.height) * scale)
        let p = viewPoint(cursor)
        cursorView.frame = CGRect(x: p.x - CGFloat(s.hotX) * scale, y: p.y - CGFloat(s.hotY) * scale, width: size.width, height: size.height)
    }

    /// 本机挪了指针：画上、发过去；放大时画面跟着指针走
    private func moveCursor(to n: CGPoint) {
        cursor = CGPoint(x: min(1, max(0, n.x)), y: min(1, max(0, n.y)))
        lastLocalMove = Date()
        screen?.send(.move(x: cursor.x, y: cursor.y))
        if zoom > 1.01 { follow() }
        layoutCursor()
    }

    /// 放大时指针靠近边上，画面往那边挪
    private func follow() {
        let p = viewPoint(cursor)
        let margin: CGFloat = 60
        var dx: CGFloat = 0, dy: CGFloat = 0
        if p.x < margin { dx = margin - p.x } else if p.x > bounds.width - margin { dx = bounds.width - margin - p.x }
        if p.y < margin { dy = margin - p.y } else if p.y > bounds.height - margin { dy = bounds.height - margin - p.y }
        guard dx != 0 || dy != 0 else { return }
        pan.x += dx
        pan.y += dy
        clampPan()
        layoutContent()
    }

    // MARK: 手势

    func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        // 两指的捏合和拖动一起认，开始后按先动的那个决定是缩放还是滚动
        let pair: Set<UIGestureRecognizer> = [pinch, twoFingerPan]
        return pair.contains(g) && pair.contains(other)
    }

    @objc private func onTap(_ g: UITapGestureRecognizer) {
        guard let screen else { return }
        if mode == .touch {
            let n = normalized(g.location(in: self))
            cursor = n
            lastLocalMove = Date()
            layoutCursor()
            screen.click(.left, at: n)
        } else {
            screen.click(.left, at: cursor)
        }
    }

    @objc private func onTwoFingerTap(_ g: UITapGestureRecognizer) {
        let n = mode == .touch ? normalized(g.location(in: self)) : cursor
        screen?.click(.right, at: n)
    }

    /// 触控板：手指挪多少，指针挪多少（快的时候多挪一点）。直接点：按住拖动
    private var dragging = false

    @objc private func onOneFingerPan(_ g: UIPanGestureRecognizer) {
        if mode == .touch {
            let n = normalized(g.location(in: self))
            switch g.state {
            case .began:
                // 手指挪过一小段才认出是拖动：从按下的地方开始拖
                let p = g.location(in: self), t = g.translation(in: self)
                let start = normalized(CGPoint(x: p.x - t.x, y: p.y - t.y))
                dragging = true
                cursor = start
                screen?.dragBegan(at: start)
                moveCursor(to: n)
            case .changed:
                moveCursor(to: n)
            default:
                if dragging { screen?.dragEnded(at: n) }
                dragging = false
            }
            return
        }
        guard g.state == .changed else { return }
        let d = g.translation(in: self)
        g.setTranslation(.zero, in: self)
        let v = g.velocity(in: self)
        let speed = hypot(v.x, v.y)
        let accel = 1 + min(1.5, speed / 1200)
        let r = contentRect
        guard r.width > 0 else { return }
        // 按没放大时的画面算，放大以后挪得慢一点、更细
        let base = r.width / zoom
        let k = accel * 1.4 / base / max(1, zoom * 0.7)
        moveCursor(to: CGPoint(x: cursor.x + d.x * k, y: cursor.y + d.y * k * (r.width / r.height)))
    }

    /// 长按：触控板模式下按住开始拖（再挪手指拖动），直接点模式下是右键
    private var pressAnchor = CGPoint.zero

    @objc private func onLongPress(_ g: UILongPressGestureRecognizer) {
        let p = g.location(in: self)
        if mode == .touch {
            if g.state == .began {
                let n = normalized(p)
                cursor = n
                layoutCursor()
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                screen?.click(.right, at: n)
            }
            return
        }
        switch g.state {
        case .began:
            pressAnchor = p
            dragging = true
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            screen?.dragBegan(at: cursor)
        case .changed:
            let r = contentRect
            guard r.width > 0 else { return }
            let k = 1.4 / (r.width / zoom) / max(1, zoom * 0.7)
            let d = CGPoint(x: p.x - pressAnchor.x, y: p.y - pressAnchor.y)
            pressAnchor = p
            moveCursor(to: CGPoint(x: cursor.x + d.x * k, y: cursor.y + d.y * k * (r.width / r.height)))
        default:
            if dragging { screen?.dragEnded(at: cursor) }
            dragging = false
        }
    }

    /// 两指：先判断这一下是缩放还是滚动
    private enum TwoFinger { case undecided, zoom, scroll }
    private var twoFinger = TwoFinger.undecided
    private var pinchStartZoom: CGFloat = 1
    private var scrollAnchor = CGPoint.zero
    private var scrollRest = CGPoint.zero

    @objc private func onPinch(_ g: UIPinchGestureRecognizer) {
        switch g.state {
        case .began:
            pinchStartZoom = zoom
        case .changed:
            if twoFinger == .undecided && abs(g.scale - 1) > 0.08 { twoFinger = .zoom }
            guard twoFinger == .zoom else { return }
            // 以两指中间为中心缩放
            let c = g.location(in: self)
            let before = contentRect
            let anchor = CGPoint(x: (c.x - before.minX) / max(1, before.width), y: (c.y - before.minY) / max(1, before.height))
            zoom = min(5, max(1, pinchStartZoom * g.scale))
            pan = .zero
            let r = contentRect
            pan = CGPoint(x: c.x - anchor.x * r.width - r.minX, y: c.y - anchor.y * r.height - r.minY)
            clampPan()
            layoutContent()
        default:
            endTwoFinger(g)
        }
    }

    @objc private func onTwoFingerPan(_ g: UIPanGestureRecognizer) {
        switch g.state {
        case .began:
            scrollAnchor = .zero
            scrollRest = .zero
        case .changed:
            let t = g.translation(in: self)
            if twoFinger == .undecided && hypot(t.x, t.y) > 12 { twoFinger = .scroll }
            if twoFinger == .zoom {
                // 缩放时两指一起挪就是平移画面
                pan.x += t.x - scrollAnchor.x
                pan.y += t.y - scrollAnchor.y
                scrollAnchor = t
                clampPan()
                layoutContent()
                return
            }
            guard twoFinger == .scroll else { return }
            let d = CGPoint(x: t.x - scrollAnchor.x, y: t.y - scrollAnchor.y)
            scrollAnchor = t
            // 自然滚动：手指往上推，内容往下滚
            let k: CGFloat = 2.5
            let at = mode == .touch ? normalized(g.location(in: self)) : cursor
            screen?.send(.wheel(dx: Double(-d.x * k), dy: Double(-d.y * k), x: at.x, y: at.y))
        default:
            endTwoFinger(g)
        }
    }

    private func endTwoFinger(_ g: UIGestureRecognizer) {
        if pinch.state != .changed && pinch.state != .began && twoFingerPan.state != .changed && twoFingerPan.state != .began {
            twoFinger = .undecided
        }
    }

    // MARK: 键盘

    func showKeyboard() { keyCatcher.becomeFirstResponder() }
    func hideKeyboard() { keyCatcher.resignFirstResponder() }
}

/// 藏起来的输入框：系统键盘（包括中文输入法）打的字从这里拿。拼音还在组字时不发，选好字才发
final class KeyCatcher: UITextField, UITextFieldDelegate {
    weak var canvas: ScreenCanvas?

    override init(frame: CGRect) {
        super.init(frame: CGRect(x: -1000, y: -1000, width: 10, height: 10))
        autocorrectionType = .no
        autocapitalizationType = .none
        spellCheckingType = .no
        smartQuotesType = .no
        smartDashesType = .no
        smartInsertDeleteType = .no
        keyboardAppearance = .dark
        returnKeyType = .default
        alpha = 0.01
        delegate = self
        addTarget(self, action: #selector(changed), for: .editingChanged)
    }

    convenience init() { self.init(frame: .zero) }
    required init?(coder: NSCoder) { fatalError() }

    @objc private func changed() {
        guard markedTextRange == nil, let s = text, !s.isEmpty else { return }
        text = ""
        MainActor.assumeIsolated { canvas?.screen?.typeText(s) }
    }

    /// 框里空着时按删除：发退格
    override func deleteBackward() {
        if (text ?? "").isEmpty {
            MainActor.assumeIsolated { canvas?.screen?.tapKey("Backspace") }
        }
        super.deleteBackward()
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        MainActor.assumeIsolated { canvas?.screen?.tapKey("Enter") }
        return false
    }
}
