use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints};
use std::io::{BufRead, BufReader, Write};
use std::fs::File;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::time::SystemTime;
use egui::{Color32, RichText};
struct CaloriApp {
    // Configuration
    t0_input: String,
    t0_val: f32,
    port_nom: String,

    // État de l'acquisition
    en_cours: bool,
    temps_debut: Option<Instant>,
    temps_ecoule_str: String,
    temp_actuelle_str: String,

    // Message de statut pour l'exportation
    statut_export: String,

    // Message de statut pour la détection du port
    statut_port: String,

    // Données partagées avec le thread de lecture série
    donnees: Arc<Mutex<Vec<(f64, f64)>>>, // (Temps en s, Température en °C)
    thread_handle: Option<std::thread::JoinHandle<()>>,
    demande_arret: Arc<Mutex<bool>>,
}

impl Default for CaloriApp {
    fn default() -> Self {
        Self {
            t0_input: "20.0".to_string(),
            t0_val: 20.0,
            port_nom: "/dev/ttyACM0".to_string(),
            en_cours: false,
            temps_debut: None,
            temps_ecoule_str: "00:00:00".to_string(),
            temp_actuelle_str: "N/A".to_string(),
            statut_export: String::new(),
            statut_port: String::new(),
            donnees: Arc::new(Mutex::new(Vec::new())),
            thread_handle: None,
            demande_arret: Arc::new(Mutex::new(false)),
        }
    }
}

/// Tente de détecter automatiquement le port série d'un Arduino.
/// Recherche les ports dont le nom ou les infos fabricant correspondent à Arduino.
/// Retourne le nom du port trouvé, ou None si aucun Arduino n'est détecté.
fn detecter_port_arduino() -> Option<String> {
    if let Ok(ports) = serialport::available_ports() {
        for port in &ports {
            // Sur Linux : /dev/ttyACM* ou /dev/ttyUSB*
            // Sur Windows : COM* avec infos fabricant Arduino
            // Sur macOS : /dev/cu.usbmodem* ou /dev/cu.usbserial*
            let nom = &port.port_name;

            // Vérification par le nom du port (Linux/macOS)
            if nom.contains("ttyACM")
                || nom.contains("ttyUSB")
                || nom.contains("usbmodem")
                || nom.contains("usbserial")
            {
                return Some(nom.clone());
            }

            // Vérification par les infos fabricant USB (Windows et Linux)
            if let serialport::SerialPortType::UsbPort(ref info) = port.port_type {
                let fabricant = info.manufacturer.as_deref().unwrap_or("").to_lowercase();
                let produit = info.product.as_deref().unwrap_or("").to_lowercase();

                if fabricant.contains("arduino")
                    || produit.contains("arduino")
                    // VID Arduino LLC = 0x2341, Arduino (www.arduino.cc) = 0x2A03
                    || info.vid == 0x2341
                    || info.vid == 0x2A03
                {
                    return Some(nom.clone());
                }
            }
        }
    }
    None
}

impl CaloriApp {
    fn lancer_acquisition(&mut self) {
        if let Ok(val) = self.t0_input.parse::<f32>() {
            self.t0_val = val;
        } else {
            self.t0_val = 0.0;
        }

        self.en_cours = true;
        self.temps_debut = Some(Instant::now());
        self.statut_export = String::new();
        *self.demande_arret.lock().unwrap() = false;

        let donnees_clone = Arc::clone(&self.donnees);
        let demande_arret_clone = Arc::clone(&self.demande_arret);
        let port_selectionne = self.port_nom.clone();
        let t0 = self.t0_val;

        self.donnees.lock().unwrap().clear();

        // Lancement du thread de lecture du port série avec gestion de la LED
        self.thread_handle = Some(std::thread::spawn(move || {
            if let Ok(mut port) = serialport::new(port_selectionne, 9600)
                .timeout(Duration::from_millis(1500))
                .open()
            {
                // Attente du reboot de l'Arduino après l'ouverture du port
                std::thread::sleep(Duration::from_secs(2));

                // Envoi du signal '1' pour allumer la LED verte et démarrer la transmission
                let _ = port.write_all(b"1");
                let _ = port.flush();

                let reader_port = port.try_clone().unwrap();
                let mut reader = BufReader::new(reader_port);
                let mut ligne = String::new();
                let temps_zero = Instant::now();

                // Horodatage du dernier point enregistré (intervalle : 1 seconde)
                let mut dernier_enregistrement = Instant::now();

                while !*demande_arret_clone.lock().unwrap() {
                    ligne.clear();
                    if reader.read_line(&mut ligne).is_ok() {
                        if !ligne.is_empty() {
                            if let Ok(delta_t) = ligne.trim().parse::<f64>() {
                                // N'enregistrer qu'un point par seconde
                                if dernier_enregistrement.elapsed() >= Duration::from_secs(1) {
                                    let temps_sec = temps_zero.elapsed().as_secs_f64();
                                    let temperature_totale = (t0 as f64) + delta_t;

                                    let mut pts = donnees_clone.lock().unwrap();
                                    pts.push((temps_sec, temperature_totale));

                                    dernier_enregistrement = Instant::now();
                                }
                            }
                        }
                    }
                }

                // Envoi du signal '0' pour éteindre la LED verte à l'arrêt
                let _ = port.write_all(b"0");
                let _ = port.flush();
            }
        }));
    }

    fn arreter_acquisition(&mut self) {
        self.en_cours = false;
        *self.demande_arret.lock().unwrap() = true;
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    fn exporter_html(&mut self) {
        let pts = self.donnees.lock().unwrap();
        if pts.is_empty() {
            self.statut_export = "Erreur : Aucune donnée à exporter !".to_string();
            return;
        }

        // --- CALCUL DE LA DATE ET DE L'HEURE EN RUST ---
        let date_formatee = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => {
                let total_secondes = n.as_secs() + 7200;
                let minutes_totales = total_secondes / 60;
                let heures_totales = minutes_totales / 60;
                let heure = heures_totales % 24;
                let minute = minutes_totales % 60;

                let jours_depuis_1970 = heures_totales / 24;
                let mut annee = 1970;
                let mut jours_restants = jours_depuis_1970;

                loop {
                    let est_bissextile = (annee % 4 == 0 && annee % 100 != 0) || (annee % 400 == 0);
                    let jours_annee = if est_bissextile { 366 } else { 365 };
                    if jours_restants < jours_annee { break; }
                    jours_restants -= jours_annee;
                    annee += 1;
                }

                let est_bissextile = (annee % 4 == 0 && annee % 100 != 0) || (annee % 400 == 0);
                let jours_par_mois = if est_bissextile {
                    vec![31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                } else {
                    vec![31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
                };

                let mut mois = 1;
                for &jours_mois in jours_par_mois.iter() {
                    if jours_restants < jours_mois { break; }
                    jours_restants -= jours_mois;
                    mois += 1;
                }

                let jour = jours_restants + 1;
                format!("{:02}/{:02}/{} -- {:02}:{:02}", jour, mois, annee, heure, minute)
            }
            Err(_) => "03/06/2026 -- 14:34".to_string(),
        };

        // --- CALCULS DES LIMITES DU GRAPHIQUE SVG ---
        let mut min_x = pts[0].0;
        let mut max_x = pts[0].0;
        let mut min_y = pts[0].1;
        let mut max_y = pts[0].1;

        for &(x, y) in pts.iter() {
            if x < min_x { min_x = x; }
            if x > max_x { max_x = x; }
            if y < min_y { min_y = y; }
            if y > max_y { max_y = y; }
        }

        let delta_x = if max_x == min_x { 10.0 } else { max_x - min_x };
        let delta_y = if max_y == min_y { 2.0 } else { max_y - min_y };

        min_x -= delta_x * 0.05;
        max_x += delta_x * 0.05;
        min_y -= delta_y * 0.1;
        max_y += delta_y * 0.1;

        let width = 1000.0;
        let height = 500.0;

        let to_svg_x = |x: f64| -> f64 { ((x - min_x) / (max_x - min_x)) * width };
        let to_svg_y = |y: f64| -> f64 { height - (((y - min_y) / (max_y - min_y)) * height) };

        let mut points_str = String::new();
        for &(x, y) in pts.iter() {
            points_str.push_str(&format!("{:.4},{:.4} ", to_svg_x(x), to_svg_y(y)));
        }

        let nom_fichier = "export_calorimetre.html";
        if let Ok(mut file) = File::create(nom_fichier) {
            let html_content = format!(
                r#"<!DOCTYPE html>
<html lang="fr">
<head>
    <meta charset="UTF-8">
    <title>Export Données Calorimètre</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; background-color: #f4f4f9; color: #333; }}
        .container {{ max-width: 1100px; margin: 0 auto; background: white; padding: 20px; border-radius: 8px; box-shadow: 0 4px 8px rgba(0,0,0,0.1); }}
        h1 {{ color: #2c3e50; text-align: center; margin-top: 0; }}
        .meta {{ margin-bottom: 20px; font-size: 14px; color: #555; text-align: center; line-height: 1.5; }}
        .btn-print {{ display: block; width: 150px; margin: 10px auto 20px auto; padding: 10px; background-color: #3498db; color: white; text-align: center; border: none; border-radius: 5px; cursor: pointer; font-weight: bold; }}
        .btn-print:hover {{ background-color: #2980b9; }}
        svg {{ width: 100%; height: auto; border: 1px solid #ddd; background-color: #fafafa; }}
        .grid {{ stroke: #e0e0e0; stroke-width: 1; stroke-dasharray: 4; }}
        .axis {{ stroke: #333; stroke-width: 2; }}
        .curve {{ fill: none; stroke: #e74c3c; stroke-width: 3; stroke-linecap: round; stroke-linejoin: round; }}
        .axis-text {{ font-size: 12px; fill: #333; font-family: sans-serif; }}
        @media print {{
            @page {{ size: landscape; margin: 1cm; }}
            body {{ margin: 0; padding: 0; background: white; }}
            .container {{ max-width: 100%; width: 100%; padding: 0; box-shadow: none; border-radius: 0; }}
            h1 {{ font-size: 20px; margin-bottom: 5px; color: #000; }}
            .meta {{ font-size: 12px; margin-bottom: 15px; color: #333; }}
            .btn-print {{ display: none; }}
            svg {{ width: 100%; height: auto; max-height: 70vh; border: 1px solid #333; background: white; page-break-inside: avoid; }}
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>Rapport de Mesure - Calorimétrie</h1>
        <div class="meta">
            Fichier généré le : {date_gen}<br>
            Nombre de points : {nb_points} | Température de base (T0) : {t0} °C
        </div>

        <button class="btn-print" onclick="window.print()">Imprimer la courbe</button>

        <svg viewBox="0 0 {width} {height}">
            <line x1="0" y1="{y_quart1}" x2="{width}" y2="{y_quart1}" class="grid" />
            <line x1="0" y1="{y_milieu}" x2="{width}" y2="{y_milieu}" class="grid" />
            <line x1="0" y1="{y_quart3}" x2="{width}" y2="{y_quart3}" class="grid" />
            <line x1="0" y1="{height}" x2="{width}" y2="{height}" class="axis" />
            <line x1="0" y1="0" x2="0" y2="{height}" class="axis" />
            <text x="10" y="20" class="axis-text">{max_y:.4} °C (Max)</text>
            <text x="10" y="{y_milieu_text}" class="axis-text">{moy_y:.4} °C</text>
            <text x="10" y="{height_minus_10}" class="axis-text">{min_y:.4} °C (Min)</text>
            <text x="5" y="{height_minus_25}" class="axis-text">{min_x:.1} s</text>
            <text x="{width_minus_80}" y="{height_minus_25}" class="axis-text">{max_x:.1} s</text>
            <polyline points="{points}" class="curve" />
        </svg>
    </div>
</body>
</html>"#,
                date_gen = date_formatee,
                nb_points = pts.len(),
                t0 = self.t0_val,
                width = width,
                height = height,
                y_quart1 = height * 0.25,
                y_milieu = height * 0.5,
                y_quart3 = height * 0.75,
                y_milieu_text = (height * 0.5) - 5.0,
                height_minus_10 = height - 10.0,
                height_minus_25 = height - 25.0,
                width_minus_80 = width - 80.0,
                max_y = max_y,
                min_y = min_y,
                moy_y = (max_y + min_y) / 2.0,
                min_x = min_x,
                max_x = max_x,
                points = points_str
            );

            if file.write_all(html_content.as_bytes()).is_ok() {
                self.statut_export = format!("Succès ! Fichier exporté sous '{}'", nom_fichier);
            } else {
                self.statut_export = "Erreur lors de l'écriture du fichier.".to_string();
            }
        } else {
            self.statut_export = "Impossible de créer le fichier.".to_string();
        }
    }
}

impl eframe::App for CaloriApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(200));

        if self.en_cours {
            if let Some(debut) = self.temps_debut {
                let d = debut.elapsed();
                let heures = d.as_secs() / 3600;
                let minutes = (d.as_secs() % 3600) / 60;
                let secondes = d.as_secs() % 60;
                self.temps_ecoule_str = format!("{:02}:{:02}:{:02}", heures, minutes, secondes);
            }

            if let Ok(pts) = self.donnees.lock() {
                if let Some(derniere_mesure) = pts.last() {
                    self.temp_actuelle_str = format!("{:.4} °C", derniere_mesure.1);
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(RichText::new("📊 Réaction de Calorimètrie en Solution").color(Color32::LIGHT_GRAY));

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Port Série :");
                ui.text_edit_singleline(&mut self.port_nom);

                // Bouton de détection automatique du port Arduino
                if ui.button("🔍 Détecter Arduino").clicked() {
                    match detecter_port_arduino() {
                        Some(port) => {
                            self.statut_port = format!("✅ Arduino détecté sur : {}", port);
                            self.port_nom = port;
                        }
                        None => {
                            self.statut_port =
                                "❌ Aucun Arduino détecté. Vérifiez le branchement USB.".to_string();
                        }
                    }
                }

                ui.label("T0 (°C) :");
                ui.add(egui::TextEdit::singleline(&mut self.t0_input).desired_width(60.0));
            });

            // Affichage du statut de détection du port
            if !self.statut_port.is_empty() {
                ui.add_space(3.0);
                if self.statut_port.starts_with("✅") {
                    ui.colored_label(egui::Color32::from_rgb(46, 204, 113), &self.statut_port);
                } else {
                    ui.colored_label(egui::Color32::from_rgb(231, 76, 60), &self.statut_port);
                }
            }

            // Affichage de la liste de tous les ports détectés sur le système
            if let Ok(ports) = serialport::available_ports() {
                let liste: Vec<String> = ports.iter().map(|p| p.port_name.clone()).collect();
                if !liste.is_empty() {
                    ui.add_space(2.0);

                    ui.horizontal(|ui| {
                        if !self.en_cours {
                            // Bouton Lancer en vert
                            let button = egui::Button::new("▶ Lancer l'Acquisition")
                                .fill(egui::Color32::from_rgb(0, 128, 0))   // Vert moyen
                                .rounding(8.0)
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 130, 70)));

                            let response = ui.add(button);

                            // Effet de survol (optionnel)
                            if response.hovered() {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            }

                            if response.clicked() {
                                self.lancer_acquisition();
                            }
                        } else {
                            // Bouton Arrêter en rouge
                            let button = egui::Button::new("⏹ Arrêter l'Acquisition")
                                .fill(egui::Color32::from_rgb(139, 0, 0))   // Rouge moyen
                                .rounding(8.0)
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 40, 30)));

                            let response = ui.add(button);

                            if response.hovered() {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            }

                            if response.clicked() {
                                self.arreter_acquisition();
                            }
                        }

                        ui.add_space(20.0);

                        // Bouton Exporter en bleu
                        let export_button = egui::Button::new("💾 Exporter la courbe (HTML/SVG)")
                            .fill(egui::Color32::from_rgb(75, 0, 130))   // Bleu
                            .rounding(8.0);

                        if ui.add(export_button).clicked() {
                            self.exporter_html();
                        }
                    });
                }
            }

            ui.add_space(10.0);



            if !self.statut_export.is_empty() {
                ui.add_space(5.0);
                if self.statut_export.starts_with("Succès") {
                    ui.colored_label(egui::Color32::from_rgb(46, 204, 113), &self.statut_export);
                } else {
                    ui.colored_label(egui::Color32::from_rgb(231, 76, 60), &self.statut_export);
                }
            }

            ui.add_space(10.0);
            ui.separator();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label("Temps écoulé :");
                    ui.heading(&self.temps_ecoule_str);
                });
                ui.add_space(50.0);
                ui.vertical(|ui| {
                    ui.label("Température actuelle :");
                    ui.heading(&self.temp_actuelle_str);
                });
            });

            ui.add_space(10.0);

            ui.label("Évolution de la Température (°C) en fonction du temps (s) :");

            let points: PlotPoints = {
                let pts = self.donnees.lock().unwrap();
                pts.iter().map(|&(x, y)| [x, y]).collect()
            };

            let ligne = Line::new(points).name("Température");

            Plot::new("calori_plot")
                .view_aspect(2.0)
                .include_y(self.t0_val as f64)
                .show(ui, |plot_ui| {
                    plot_ui.line(ligne);
                });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 900.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Calorimétre en solution",
        options,
        Box::new(|_cc| Box::new(CaloriApp::default())),
    )
}