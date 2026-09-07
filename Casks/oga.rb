cask "oga" do
  version "0.0.8"
  sha256 "da750618084f79e458ec261da8e4df9d23d9f5f2f38d3c24317a2303fe4f7dd9"

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
