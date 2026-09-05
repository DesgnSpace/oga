cask "oga" do
  version "0.0.6"
  sha256 "8a887feacbf3d53e73e343d3442ba7441cbc17f995cd2cc0426dfbd461b588b0"

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
