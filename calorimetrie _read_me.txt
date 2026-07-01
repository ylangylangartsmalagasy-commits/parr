Étape 1 : Permissions pour le port série

Sous Linux, par défaut, l'accès aux ports de type /dev/ttyACM0 requiert des droits spécifiques. Donnez-vous l'accès au port en ajoutant votre utilisateur au groupe dialout (ou uucp selon la distribution) :
Bash

sudo usermod -aG dialout $USER

(Note : Il faut fermer et rouvrir votre session Linux pour que le changement s'applique).
Étape 2 : Dépendances graphiques

egui s'appuie sur des bibliothèques systèmes pour l'affichage (X11/Wayland et OpenGL). Installez-les (exemple pour Ubuntu/Debian) :
Bash

sudo apt update
sudo apt install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
