cask "oga" do
  version "0.2.5"
  sha256 "27f91f66f1978f0bb21ca6a41860ac4ee42bb19d70897152ef2b1bff4e6553e1"

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
