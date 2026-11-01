cask "oga" do
  version "0.2.4"
  sha256 "81726952fcd20d7885059a96acccc081c56f5bb6f8ee05855d5420c59b2eaf25"

  url "https://downloads.desgn.space/oga/Oga-#{version}.zip"
  name "Oga"
  desc "Local broker for delegating bounded tasks to AI provider CLIs"
  homepage "https://oga.desgn.space"

  livecheck do
    url "https://downloads.desgn.space/oga/releases.json"
    strategy :json do |json|
      json["latest"]
    end
  end

  depends_on macos: ">= :sonoma"

  app "Oga.app"
  binary "#{appdir}/Oga.app/Contents/MacOS/oga-server", target: "oga"
end
