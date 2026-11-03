// Renders an EPUB XHTML with WebKit (Apple Books' engine) at a given
// width and saves a PNG capture. Usage: swift snap.swift <file.xhtml> <width> <height> <output.png> [extra css]
// The style sheets that Apple Books injects into every book (BookEPUB.framework)
// are added if present, with its default settings (automatic hyphenation
// in French), for a rendering as close as possible to the app.
import Cocoa
import WebKit

let args = CommandLine.arguments
let url = URL(fileURLWithPath: args[1])
let w = Double(args[2])!, h = Double(args[3])!
let out = args[4]
let extraCss = args.count > 5 ? args[5] : ""
let booksRes = "/System/Applications/Books.app/Contents/Frameworks/BookEPUB.framework/Versions/A/Resources/"
var appleCss = ":root { --ibooks-hyphens: auto; --ibooks-locale: fr; --background-color: #fff; --text-color: #000; }\n"
for name in ["user_stylesheet_base", "user_stylesheet_flowable", "user_stylesheet_fonts", "user_stylesheet_colors_light"] {
    if let css = try? String(contentsOfFile: booksRes + name + ".css", encoding: .utf8) {
        // @namespace is only valid at the top of a sheet: we remove them.
        appleCss += css.split(separator: "\n").filter { !$0.hasPrefix("@namespace") }.joined(separator: "\n") + "\n"
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let config = WKWebViewConfiguration()
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: w, height: h), configuration: config)
let window = NSWindow(contentRect: web.frame, styleMask: [.borderless], backing: .buffered, defer: false)
window.contentView = web

class D: NSObject, WKNavigationDelegate {
    func webView(_ v: WKWebView, didFinish nav: WKNavigation!) {
        // Apple Books' sheet first (like a user sheet), then our settings.
        let all = (appleCss + extraCss).replacingOccurrences(of: "`", with: "\\`")
        let js = "var s=document.createElement('style');s.textContent=`\(all)`;document.head.insertBefore(s, document.head.firstChild);document.documentElement.lang='fr';'ok'"
        v.evaluateJavaScript(js) { _, _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                let cfg = WKSnapshotConfiguration()
                v.takeSnapshot(with: cfg) { img, err in
                    if let img = img, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff), let png = rep.representation(using: .png, properties: [:]) {
                        try? png.write(to: URL(fileURLWithPath: out))
                        print("écrit \(out) \(img.size)")
                    } else { print("échec : \(String(describing: err))") }
                    app.terminate(nil)
                }
            }
        }
    }
}
let d = D()
web.navigationDelegate = d
web.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent().deletingLastPathComponent())
app.run()
