import SwiftUI

struct NativePageScaffold<HeaderAccessory: View, Content: View>: View {
    let title: String
    @ViewBuilder var headerAccessory: HeaderAccessory
    @ViewBuilder var content: Content

    init(
        title: String,
        @ViewBuilder headerAccessory: () -> HeaderAccessory,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.headerAccessory = headerAccessory()
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                HStack(spacing: 12) {
                    Text(title)
                        .font(.system(size: 30, weight: .bold))
                    headerAccessory
                }
                .padding(.top, 20)
                content
            }
            .padding(.horizontal, 36)
            .padding(.bottom, 36)
            .frame(maxWidth: 1180, alignment: .leading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(AppSurface.content)
    }
}

extension NativePageScaffold where HeaderAccessory == EmptyView {
    init(title: String, @ViewBuilder content: () -> Content) {
        self.init(title: title, headerAccessory: { EmptyView() }, content: content)
    }
}

struct NativePanel<Content: View>: View {
    var scaleOnHover: CGFloat = 1.004
    @ViewBuilder var content: Content
    @State private var isHovering = false

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(AppSurface.card)
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
            .shadow(color: AppSurface.cardGlow, radius: isHovering ? 22 : 12, x: 0, y: 0)
            .shadow(color: isHovering ? AppSurface.hoverShadow : AppSurface.cardShadow, radius: isHovering ? 18 : 10, x: 0, y: isHovering ? 10 : 5)
            .scaleEffect(isHovering ? scaleOnHover : 1)
            .animation(.easeOut(duration: 0.16), value: isHovering)
            .onHover { isHovering = $0 }
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
