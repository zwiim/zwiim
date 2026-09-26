Pod::Spec.new do |s|
  s.name           = 'ZwiimLan'
  s.version        = '0.1.0'
  s.summary        = 'TCP connections to a Zwiim host on the local network'
  s.description    = 'TCP connections to a Zwiim host on the local network, for the SDK LAN path'
  s.author         = 'Zwiim'
  s.homepage       = 'https://zwiim.dev'
  s.platforms      = {
    :ios => '16.4'
  }
  s.source         = { git: '' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  # Swift/Objective-C compatibility
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
  }

  s.source_files = "**/*.{h,m,mm,swift,hpp,cpp}"
end
