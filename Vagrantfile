# -*- mode: ruby -*-
# vi: set ft=ruby :

Vagrant.configure(2) do |config|
  config.vm.box = "precise64"
  config.vm.provision "shell", inline: "sudo apt-get update && sudo apt-get install build-essential"
end
