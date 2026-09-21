cask "oga" do
  version "0.2.1"
  sha256 "a3e085a9bec11bc4625648f4cafc37b0d2bc8c603510de5099311d3a95ac545c"

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
