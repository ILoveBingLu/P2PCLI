import AVFoundation
import PhotosUI
import SwiftUI
import Vision
import VisionKit

/// 扫码连接：扫电脑上 P2PCLI 显示的二维码（p2pcli://pair?…），里面有设备码和配对密码。
/// 用系统的扫码控件：iPhone Duo 开合、换屏时它自己换摄像头。扫不了（模拟器、没给相机权限）时可以从相册选二维码图片
struct ScanScreen: View {
    @Environment(Core.self) private var core
    @State private var permission = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var message: String?
    @State private var done = false
    @State private var photo: PhotosPickerItem?

    var body: some View {
        let canScan = DataScannerViewController.isSupported && permission == .authorized
        ZStack {
            Color.black.ignoresSafeArea()
            if canScan {
                // 只放相机画面：系统扫码控件认到码时自己会框出来，不另画取景框
                Scanner { handle($0) }.ignoresSafeArea()
            } else {
                VStack(spacing: 12) {
                    Icon(permission == .denied || permission == .restricted ? "triangle-alert" : "scan-line", size: 44)
                        .foregroundStyle(Hero.muted)
                    if permission == .denied || permission == .restricted {
                        Text(t("没有相机权限")).font(.title3.weight(.semibold))
                        Text(t("在系统设置里允许 P2PCLI 使用相机，或者输入设备码连接。"))
                            .font(.subheadline).foregroundStyle(Hero.muted).multilineTextAlignment(.center)
                        Button(t("去设置")) {
                            if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                        }
                        .heroProminent()
                    } else if permission == .authorized {
                        Text(t("这台设备不能用相机扫码")).font(.title3.weight(.semibold))
                        Text(t("从相册选一张二维码截图，或者输入设备码连接。"))
                            .font(.subheadline).foregroundStyle(Hero.muted).multilineTextAlignment(.center)
                    }
                }
                .padding(32)
                .frame(maxWidth: 420)
            }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 14) {
                Text(message ?? t("对准电脑上 P2PCLI 显示的二维码"))
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 16).padding(.vertical, 10)
                    .heroCapsuleGlass()
                GlassGroup(spacing: 10) {
                    HStack(spacing: 10) {
                        PhotosPicker(selection: $photo, matching: .images) {
                            Label { Text(t("从相册选择")) } icon: { Icon("image", size: 18) }
                                .padding(.horizontal, 16).padding(.vertical, 12)
                        }
                        .buttonStyle(.plain)
                        .heroCapsuleGlass(interactive: true)
                        Button { core.scanToManual() } label: {
                            Label { Text(t("输入设备码")) } icon: { Icon("key-round", size: 18) }
                                .padding(.horizontal, 16).padding(.vertical, 12)
                        }
                        .buttonStyle(.plain)
                        .heroCapsuleGlass(interactive: true)
                    }
                }
                .foregroundStyle(.white)
            }
            .padding(.bottom, 20)
        }
        .overlay(alignment: .topLeading) {
            Button { core.scanning = false } label: {
                Icon("x", size: 20).foregroundStyle(.white).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .heroCapsuleGlass(interactive: true)
            .padding(16)
            .accessibilityLabel(t("关闭扫码"))
        }
        .task {
            if permission == .notDetermined {
                _ = await AVCaptureDevice.requestAccess(for: .video)
                permission = AVCaptureDevice.authorizationStatus(for: .video)
            }
        }
        .onChange(of: photo) { _, item in
            guard let item else { return }
            Task {
                photo = nil
                guard let data = try? await item.loadTransferable(type: Data.self) else { return }
                let codes = await Task.detached { Self.decode(data) }.value
                if codes.isEmpty {
                    message = t("这张图片里没找到二维码")
                } else {
                    for c in codes where handle(c) { break }
                }
            }
        }
    }

    /// 认出一个二维码：是 P2PCLI 的就去连，返回真
    @discardableResult
    private func handle(_ text: String) -> Bool {
        guard !done else { return true }
        guard let pair = parsePairLink(text: text) else {
            message = t("这不是 P2PCLI 的连接二维码")
            return false
        }
        done = true
        message = t("已识别，正在连接…")
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        core.scanned(pair)
        return true
    }

    private nonisolated static func decode(_ data: Data) -> [String] {
        guard let image = CIImage(data: data) else { return [] }
        let request = VNDetectBarcodesRequest()
        request.symbologies = [.qr]
        try? VNImageRequestHandler(ciImage: image).perform([request])
        return (request.results ?? []).compactMap(\.payloadStringValue)
    }
}

/// 系统的扫码控件
private struct Scanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced,
                                           recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false,
                                           isGuidanceEnabled: false, isHighlightingEnabled: true)
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {
        context.coordinator.onCode = onCode
    }

    static func dismantleUIViewController(_ vc: DataScannerViewController, coordinator: Coordinator) {
        vc.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        var onCode: (String) -> Void
        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if case .barcode(let code) = item, let s = code.payloadStringValue { onCode(s) }
            }
        }
    }
}
