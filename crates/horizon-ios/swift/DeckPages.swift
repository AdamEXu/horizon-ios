// iPhone Duo held upright: the page view below the fold. SwiftUI owns the paging (its physics,
// the page's title and the page dots); the game draws each page's content under it, at the
// offset the pager reports (game/deck.rs), so items, the map and the 3D player come from the
// game's renderer. The Rust app drives it through the C functions at the bottom, on the main
// thread (winit's event loop).
import SwiftUI
import UIKit

@MainActor
final class DeckModel: ObservableObject {
    @Published var titles: [String] = []
    @Published var page: Int? = 0
    /// The pager's position in pages (1.5: halfway from the second page to the third).
    var offset: Double = 0
    /// Taps on a page: the page, and where on it (points).
    var taps: [(Int, CGPoint)] = []
}

/// The bands the title and the dots take, in points: the game keeps its content between them.
let titleBand: CGFloat = 44
let dotsBand: CGFloat = 30
/// The card's corners, as the deck draws its recess (game/deck.rs `CARD_CORNER`).
let cardCorner: CGFloat = 26

/// Follows the pager as it scrolls (iOS 18), else as it settles on a page.
private struct OffsetTracker: ViewModifier {
    let model: DeckModel
    let width: CGFloat

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content
                .onScrollGeometryChange(for: CGFloat.self) { geometry in
                    geometry.contentOffset.x + geometry.contentInsets.leading
                } action: { _, x in
                    model.offset = Double(x / max(1, width))
                }
                // A relayout (the fold closing and opening) can leave the pager between pages
                // without a scroll: settle the offset on the page it shows.
                .onChange(of: model.page) { _, page in
                    model.offset = Double(page ?? 0)
                }
                .onChange(of: width) { _, _ in
                    model.offset = Double(model.page ?? 0)
                }
        } else {
            content.onChange(of: model.page) { _, page in
                model.offset = Double(page ?? 0)
            }
        }
    }
}

struct DeckPagesView: View {
    @ObservedObject var model: DeckModel

    var body: some View {
        GeometryReader { outer in
            ScrollView(.horizontal) {
                HStack(spacing: 0) {
                    ForEach(model.titles.indices, id: \.self) { index in
                        Color.clear
                            .contentShape(Rectangle())
                            .containerRelativeFrame(.horizontal)
                            .onTapGesture(coordinateSpace: .local) { point in
                                model.taps.append((index, point))
                            }
                            .id(index)
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.paging)
            .scrollIndicators(.hidden)
            .scrollPosition(id: $model.page)
            .modifier(OffsetTracker(model: model, width: outer.size.width))
            .overlay(alignment: .top) { title }
            .overlay(alignment: .bottom) { dots }
        }
        .ignoresSafeArea()
        .clipShape(RoundedRectangle(cornerRadius: cardCorner, style: .continuous))
    }

    private var current: Int {
        min(max(model.page ?? 0, 0), max(model.titles.count - 1, 0))
    }

    private var title: some View {
        Text(model.titles.isEmpty ? "" : model.titles[current])
            .font(.headline)
            .foregroundStyle(.white)
            .padding(.horizontal, 14)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
            .environment(\.colorScheme, .dark)
            .frame(height: titleBand)
            .animation(.snappy, value: current)
            .allowsHitTesting(false)
    }

    private var dots: some View {
        HStack(spacing: 8) {
            ForEach(model.titles.indices, id: \.self) { index in
                Circle()
                    .fill(.white.opacity(index == current ? 0.95 : 0.3))
                    .frame(width: 7, height: 7)
                    .onTapGesture {
                        withAnimation(.snappy) { model.page = index }
                    }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial, in: Capsule())
        .environment(\.colorScheme, .dark)
        .frame(height: dotsBand)
    }
}

@MainActor private let model = DeckModel()
@MainActor private var host: UIHostingController<DeckPagesView>?

/// Puts the page view on `view` (winit's UIView), once.
@_cdecl("horizon_deck_attach")
public func horizonDeckAttach(_ view: UnsafeMutableRawPointer) {
    MainActor.assumeIsolated {
        guard host == nil else { return }
        let parent = Unmanaged<UIView>.fromOpaque(view).takeUnretainedValue()
        let controller = UIHostingController(rootView: DeckPagesView(model: model))
        controller.view.backgroundColor = .clear
        controller.view.isOpaque = false
        controller.view.isHidden = true
        controller.safeAreaRegions = []
        parent.addSubview(controller.view)
        host = controller
    }
}

/// Where the page view is on winit's view (points), or hidden.
@_cdecl("horizon_deck_layout")
public func horizonDeckLayout(_ x: Double, _ y: Double, _ width: Double, _ height: Double, _ visible: Bool) {
    MainActor.assumeIsolated {
        guard let view = host?.view else { return }
        view.isHidden = !visible
        let frame = CGRect(x: x, y: y, width: width, height: height)
        if visible, view.frame != frame {
            view.frame = frame
            model.offset = Double(model.page ?? 0)
        }
    }
}

/// The pages' titles, one a line (translated by the game).
@_cdecl("horizon_deck_titles")
public func horizonDeckTitles(_ text: UnsafePointer<CChar>) {
    MainActor.assumeIsolated {
        let titles = String(cString: text).split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if model.titles != titles { model.titles = titles }
    }
}

/// The pager's position in pages.
@_cdecl("horizon_deck_offset")
public func horizonDeckOffset() -> Double {
    MainActor.assumeIsolated { model.offset }
}

/// The bands over the pages' content (points): the title's at the top, the dots' at the bottom.
@_cdecl("horizon_deck_bands")
public func horizonDeckBands(_ top: UnsafeMutablePointer<Double>, _ bottom: UnsafeMutablePointer<Double>) {
    top.pointee = Double(titleBand)
    bottom.pointee = Double(dotsBand)
}

/// The oldest tap on a page not yet taken: its page and where on it (points). False when none.
@_cdecl("horizon_deck_next_tap")
public func horizonDeckNextTap(
    _ page: UnsafeMutablePointer<Int32>, _ x: UnsafeMutablePointer<Double>, _ y: UnsafeMutablePointer<Double>
) -> Bool {
    MainActor.assumeIsolated {
        guard !model.taps.isEmpty else { return false }
        let (index, point) = model.taps.removeFirst()
        page.pointee = Int32(index)
        x.pointee = Double(point.x)
        y.pointee = Double(point.y)
        return true
    }
}
