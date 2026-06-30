import AppKit
import SwiftUI

enum SidebarItem: String, CaseIterable, Identifiable {
    case network = "Network"
    case replay = "Replay"
    case rules = "Rules"
    case values = "Values"
    case scripts = "Scripts"
    case ai = "AI"
    case devTools = "DevTools"
    case groups = "Groups"
    case notify = "Notify"
    case settings = "Settings"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .network: return "globe"
        case .replay: return "bolt"
        case .rules: return "doc.text"
        case .values: return "server.rack"
        case .scripts: return "terminal"
        case .ai: return "face.smiling"
        case .devTools: return "ladybug"
        case .groups: return "person.2.badge.gearshape"
        case .notify: return "bell"
        case .settings: return "gearshape"
        }
    }
}

struct Sidebar: View {
    @Binding var selection: SidebarItem
    @EnvironmentObject private var appModel: AppModel

    private let sidebarWidth: CGFloat = 72

    var body: some View {
        VStack(spacing: 0) {
            WindowControlButtons()
                .frame(width: sidebarWidth, height: 42)

            ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 5) {
                    ForEach(SidebarItem.allCases) { item in
                        SidebarButton(
                            item: item,
                            isSelected: selection == item,
                            action: { selection = item }
                        )
                    }
                }
                .padding(.top, 6)
                .padding(.bottom, 10)
            }
            .frame(maxHeight: .infinity)

            Button {
                appModel.colorSchemeMode = appModel.colorSchemeMode.next
            } label: {
                VStack(spacing: 4) {
                    Image(systemName: appModel.colorSchemeMode.systemImage)
                        .font(.system(size: 19, weight: .medium))
                        .frame(width: 26, height: 24)
                    Text(appModel.colorSchemeMode.rawValue)
                        .font(.system(size: 10, weight: .medium))
                        .lineLimit(1)
                }
                .frame(width: 56, height: 52)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Toggle \(appModel.colorSchemeMode.next.rawValue) Theme")
            .padding(.bottom, 10)
        }
        .frame(width: sidebarWidth)
        .background(SidebarRailBackground())
        .ignoresSafeArea(.container, edges: [.top, .bottom])
    }
}

private struct WindowControlButtons: View {
    @State private var isHovering = false

    var body: some View {
        HStack(spacing: 8) {
            trafficLight(color: Color(red: 1.0, green: 0.37, blue: 0.34), symbol: "xmark") {
                withMainWindow { $0.performClose(nil) }
            }
            trafficLight(color: Color(red: 1.0, green: 0.74, blue: 0.0), symbol: "minus") {
                withMainWindow { $0.miniaturize(nil) }
            }
            trafficLight(color: Color(red: 0.20, green: 0.78, blue: 0.35), symbol: "plus") {
                withMainWindow { $0.zoom(nil) }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(.top, 13)
        .padding(.leading, 12)
        .background(SidebarRailBackground())
        .onHover { isHovering = $0 }
    }

    private func trafficLight(color: Color, symbol: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            ZStack {
                Circle()
                    .fill(color)
                    .frame(width: 14, height: 14)
                if isHovering {
                    Image(systemName: symbol)
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(Color.black.opacity(0.45))
                }
            }
            .frame(width: 14, height: 14)
            .contentShape(Circle())
        }
        .buttonStyle(.plain)
    }

    private func withMainWindow(_ action: (NSWindow) -> Void) {
        guard let window = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.title == "Bifrost" }) else {
            return
        }
        action(window)
    }
}

struct SidebarRailBackground: View {
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Rectangle()
            .fill(backgroundColor)
    }

    private var backgroundColor: Color {
        Color(nsColor: Self.nsColor(for: colorScheme))
    }

    static func nsColor(for colorScheme: ColorScheme) -> NSColor {
        switch colorScheme {
        case .dark:
            return NSColor(calibratedWhite: 0.13, alpha: 1)
        default:
            return NSColor(calibratedRed: 0.965, green: 0.982, blue: 1.0, alpha: 1)
        }
    }
}

private struct SidebarButton: View {
    let item: SidebarItem
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack(alignment: .topLeading) {
                if isSelected {
                    Rectangle()
                        .fill(Color.accentColor)
                        .frame(width: 3, height: 46)
                        .clipShape(.rect(bottomTrailingRadius: 2, topTrailingRadius: 2))
                }

                VStack(spacing: 3) {
                    ZStack(alignment: .topTrailing) {
                        Image(systemName: item.systemImage)
                            .font(.system(size: 20, weight: .medium))
                            .frame(width: 28, height: 25)
                        if item == .notify {
                            Text("99+")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(.white)
                                .padding(.horizontal, 5)
                                .padding(.vertical, 1)
                                .background(Color.red, in: Capsule())
                                .offset(x: 13, y: -8)
                        }
                    }

                    Text(item.rawValue)
                        .font(.system(size: 10, weight: .medium))
                        .lineLimit(1)
                        .minimumScaleFactor(0.72)
                        .frame(width: 58)
                }
                .frame(width: 64, height: 54)
                .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                .background(isSelected ? Color.accentColor.opacity(0.13) : Color.clear, in: RoundedRectangle(cornerRadius: 8))
            }
            .frame(width: 64, height: 54)
            .contentShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .help(item.rawValue)
    }
}
