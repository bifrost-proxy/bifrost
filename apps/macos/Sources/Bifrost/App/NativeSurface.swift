import SwiftUI

struct NativePageScaffold<HeaderAccessory: View, Content: View>: View {
    let title: String
    let contentFillsAvailableHeight: Bool
    @ViewBuilder var headerAccessory: HeaderAccessory
    @ViewBuilder var content: Content

    init(
        title: String,
        contentFillsAvailableHeight: Bool = false,
        @ViewBuilder headerAccessory: () -> HeaderAccessory,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.contentFillsAvailableHeight = contentFillsAvailableHeight
        self.headerAccessory = headerAccessory()
        self.content = content()
    }

    var body: some View {
        GeometryReader { proxy in
            let horizontalPadding = pageHorizontalPadding(for: proxy.size.width)
            if contentFillsAvailableHeight {
                pageStack
                    .padding(.horizontal, horizontalPadding)
                    .padding(.bottom, 36)
                    .frame(maxWidth: .infinity, minHeight: proxy.size.height, alignment: .topLeading)
            } else {
                ScrollView {
                    pageStack
                        .padding(.horizontal, horizontalPadding)
                        .padding(.bottom, 36)
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            AppSurface.content
            WindowDragBlocker()
        }
    }

    private var pageStack: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 12) {
                Text(title)
                    .font(.system(size: 30, weight: .bold))
                headerAccessory
            }
            .padding(.top, 10)
            content
                .frame(
                    maxWidth: .infinity,
                    maxHeight: contentFillsAvailableHeight ? .infinity : nil,
                    alignment: .topLeading
                )
        }
    }

    private func pageHorizontalPadding(for width: CGFloat) -> CGFloat {
        if width < 720 {
            return 20
        }
        if width < 980 {
            return 28
        }
        return 36
    }
}

extension NativePageScaffold where HeaderAccessory == EmptyView {
    init(title: String, contentFillsAvailableHeight: Bool = false, @ViewBuilder content: () -> Content) {
        self.init(
            title: title,
            contentFillsAvailableHeight: contentFillsAvailableHeight,
            headerAccessory: { EmptyView() },
            content: content
        )
    }
}

struct NativePanel<Content: View>: View {
    var scaleOnHover: CGFloat = 1.004
    var allowsHoverEffect = true
    @ViewBuilder var content: Content
    @State private var isHovering = false

    private var effectiveHover: Bool {
        allowsHoverEffect && isHovering
    }

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(AppSurface.card)
                WindowDragBlocker()
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(AppSurface.cardBorder)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(
                        LinearGradient(
                            colors: [
                                AppSurface.cardHighlight,
                                AppSurface.cardHighlight.opacity(0.45),
                                AppSurface.cardBorder,
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        ),
                        lineWidth: 1
                    )
            )
            .shadow(color: AppSurface.cardGlow, radius: effectiveHover ? 22 : 12, x: 0, y: 0)
            .shadow(color: effectiveHover ? AppSurface.hoverShadow : AppSurface.cardShadow, radius: effectiveHover ? 18 : 10, x: 0, y: effectiveHover ? 10 : 5)
            .scaleEffect(effectiveHover ? scaleOnHover : 1)
            .animation(.easeOut(duration: 0.16), value: isHovering)
            .onHover { isHovering = allowsHoverEffect && $0 }
    }
}

struct NativeCard<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        NativePanel {
            content
                .padding(18)
        }
    }
}

struct NativeCardHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .font(.system(size: 15, weight: .semibold))
            Text(subtitle)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
    }
}

struct CompactFact: View {
    let title: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 15, weight: .semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.7)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AppSurface.subtleFill, in: RoundedRectangle(cornerRadius: 7))
    }
}

struct AdaptiveFactGrid<Content: View>: View {
    var minimum: CGFloat = 118
    var maximum: CGFloat = 220
    var spacing: CGFloat = 10
    @ViewBuilder var content: Content

    var body: some View {
        LazyVGrid(
            columns: [
                GridItem(.adaptive(minimum: minimum, maximum: maximum), spacing: spacing, alignment: .topLeading)
            ],
            alignment: .leading,
            spacing: spacing
        ) {
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct StatusPill: View {
    let title: String
    let color: Color

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            Text(title)
                .font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(.secondary)
    }
}

struct EmptyNativeState: View {
    let title: String

    var body: some View {
        VStack(spacing: 8) {
            Text(title)
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
