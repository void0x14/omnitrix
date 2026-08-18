Web mcpleri ile webi araştır ve bizim stacklerimiz ile uyumlu,efektif,ram dostu,7/24 otonom sürekli çalışmada belleği şişirmeyen,hata vermeyen,bloat olmayan ui/ux/tui/webui araştırması yapacaksın

İnşaa etmek istediğimiz araç;7/24 otonom çalışan,native tui'si olan,ama yine native şekilde webui'dan da her şekilde takip edilebilen/kontrol edilebilen yani tuide yapabildiğin her şeyi webui'da da yapabildiğin bir araç.
Bu aracın başlıca istediğim özellikleri şunlar olacak;

- [x] Çoklu sağlayıcı (provider) desteği: 2 şekilde çalışacak sistem; 1| Ben bir api key girdiğimde,o api keyin hangi sağlayıcıya ait olduğunu otomatik tespit edip,sisteme kaydedecek. 2| Ben provider listesinden providerı seçip,api keyimi girecem.

- [] Provider listesinde aktif providerlar yani key girilen api key girile nproviderların yanında [KEY] yazıyor.
    Ben bunu ıstemıyorum,ben o  çapraz duran karenın yeşil yanmasını istiyorum.Yani api key girili providerlarda key yazmayacak,yeşil yanacak o kare.

- [] Yükü dağıtma/fallback/Yargıç(judge) mimari desteği: Sistemimizdeki api keylerce elde edilen Ai modellerini 3 farklı şekilde kullanabileceğiz. 1| Route-balance/round-balance şeklinde (Yükü,elimizdeki api keylerin tamamına dağıtarak,maksimize ediyoruz),2| fallback mekanızmasında ise bir api key hata verdiğinde (örn: bakiyesi bitti,baska hatalar vs),diğer çalışan api keye geçip onunla yoluna devam etmesi.Sırayla dene yukardan aşağı api keyleri,hangisi çalısıyosa onla devam eder. 3| Yargıç olarak claude sonnet 5 modelini kullanır,ardından executer olarak deepseek v4 flash kullanır,plan ve görevleri belirleme seçme olarakda grok 4.5 kullanır (Yargıçta dahil olmak üzere,bütün modeller istediğimiz gibi değiştirilebilir olacak).Buradaki amaç ise tamamen daha iyi sistem.Evet daha pahalı ve cok token harcıyor ama yınede iş görüyor.

- [] Sqlite db'den veri alabilecek: Hem gerçek zamanlı,hem de pasif zamanlı şekilde sqlite databaseden veri alabilecek ve bu aldığı verileride yine doğru providerlara ekleyecek.Eklemeden öncede canlılık kontrolü yapacak.Bu sayede canlı olmayanları eklemeyecek ve cansız diye farklı bir bölüme atacak onları (çünkü ileride canlanabilir diye elde tutuyoruz)

- [] Dil agnostik: Programlama dili farketmeksizin her zaman istediğimiz gibi hızlıca özellik ekleyip/çıkarabilmeli,hata olduğunda hızlıca tespit edip fixleyebilmeli,gizli senaryolar durumlar olmamalı/yaşanmamalı.İstenildiği vakit sisteme uzaktan bağlanılabilir,istenildiği vakit androidden sistem kontrol edilebilir ya da izlenilebilir.

- [] Ram dostu: Hem soğuk başlangıçta,hem de çalıştığı süre boyunca (runtimeda),ram kullanımı sürekli artmamalı ve minimal düzeyde olmalıdır.Benim hayalimdeki max ram kullanımı 250MB,ama tabi mümkünse böyle 2 haneli rakamlara da inmek isterim. Çünkü bu tarz araçlarda ram kullanımı kritiktir.
Kritik olma sebebide bu tarz programlar ileride çok çok featurelar alır,çok sayıda anlık işlem yapar,gerçek zamanlı/apsif zamanlı.Bu da ister istemez ramin içinden geçer.O yüzden biz çekirdek ve çekirdek özellikleri ne kadar az ram tüketir yapabilir/tasarlayabilirsek,üzerine koyacağımız şeylere o kadar fazla alan açılır

- [] Cold start hızlı olmalı: Yani programı ilk kez başlatsam bile oldukça hızlı açılmalı.Ben gecikmeyi farketmemeliyim bile,insan algısının anlayamayacağı bir gecikmeye sahip olmalı.

- [] Programı kapatmak istediğimde anında kapanması: Program isterse 100GB veri okuma/yazma yapsın,hiç farketmeksizin hiç bir data kaybı olmadan,ben ctrl+c yaptığımda,ZINK diye anında kapanmalı.Ben insan algımla herhangi bir gecikme ve bekleme hissetmemeliyim.

- [] Native multiagent mimarisi & agentlar hakkında tam kontrol: Programımız yeri gelecek aynı anda 10/100/1000/10000 multi ajan açacak/görevlendirecek.Yine yeri gelecek bu görevlendirdiği multiajanların her biriside yine kendi iradesiyle istediği x kadar multiajan görevlendirebilecek,yine o görevlendirdiği multiajanlarda istediği x kadar görevlendirebilecek ama ondan sonrası görevlendiremöeyecek.Yani belli bi sınır var diyebilirim.
Görevlendirilen her bir multiajana,bu ai'lar tam kontrol sahibi olacak.Onları gerçek zamanlı 7/24 durumlarını takip edecek,kurallara uyup uymadıklarını dnetleyecek,uymadıysa interrupt edip burayı hatalı yaptın xxx gibi uyaracak kufretcek cezalandırcak.Yani subagent mımarısı gıbı öyle açtım habersız ve asla tekrar ona yanıt yazamıyorum gibi bir şey olmayacak.
Bütün multiajanlara hem ailar hem de ben yazı yazabilecem.Yani prompt verebilecem.
Ek olarak bütün multiajanları ben ayrı bir tabde görebileceğim.İşte görevi devam edenleri,görevini bitirmiş olanları falanı filanı.Burası detaylandırılır,daha aklımda 100 den fazla nokta var ama yazamam cok uzun olur.

- [] Döngü mühendisliği: Sistem tam otonom olacak dediğim gibi.Buna keşif aşamasıda dahil aklına gelen her aşamada.Ben yani bizler yalnızca bu döngüleri kusursuz bir şekilde tasarlaıyp kuracağız,ardından bir kez çalıştıracağız ve o sonsuza dek çalışıp işini yapacak,ta ki ne zaman istenilen görevi istenilen şekilde tamamlar,beklenilen çıktıyı/sonucu getirir,ancak o zaman sonlanacak.
Buradada yine 2 farklı döngü tipi olacak: Birisi tam otonom (pahalı ama inanılmaz iterasyon yapabiliyor),diğeride kullanıcı odaklı.Yani kullanıcı odaklıda,görev tanımını osunu busunu ve çoğu şeyi ben verıyorum,o bunlar dahilinde görevı yerıne getırıyor ve bütçe dostu oluyor.Ama diğerinde ben sadece problemı verıyorum,bunu çöz,şöyle görmek ıstıyorum dıyorum atıyorum,ajan kendısı sonsuz sayıda ıterasyon,sonsuz sayıda multıajan açarak,ajan bütün süreci a'dan z'ye kendisi yürütüyor,planlıyor,araştırıyor,fixliyor,çözüyor.

- [] Ai'ların dalkavukluğu ve yalancılığını çözme: İstisnasız bütün ai'lar yalan söylüyor.Sadece bazıları bunu daha az yapıyor,bazıları daha çok.Ben ise bunu istemiyorum.Benim sürecim programım tamamen determinist ve gerçek odaklı olmak ZORUNDA. Yani bir ai gerçek dışına asla çıkamaz,asla kendi gerçeğini oluşturup orada kum havuzunda oynar gibi oynayamaz.Her zaman benim istediğim şekilde her şeye yaklaşmalı ve ilerlemeli.Peki ben gerçeği çarpıtmıyor muyım dıye sorabilirsin ama çarpıtmıyorum.Benim yöntemlerımle ilerler,baktı benım yontemlerım işe yaramadı çözemedi işi.O zaman gider yenı yontemler bulur,kendı ıterasyonlarını dener vs vs.
Her zaman yerleşik araçları kullanmak zorundalar.Örneğin bizim aracımızın yerleşik toollarından edit/write/search vs var diyelim,onları kullanmak yerine tembellik edip grep kullandı,ya da cat kullandı,ya da sed kullanarak yazdı okudu,pyeof falan gıbı eof kullanarak iş yaptı.İşte sistemimiz direk tbunu engelleyecek.Yani llm bunu uretemyecek bıle öyle bişey dusunebılırız.
Yalnızca bizim araçlarımızı kullanarak işini tamamlamak zorunda.Çok çok çok zaruri durumlarda,bizim araçlarımız %99 offline olduğu,çalışamadığı ve çözülemediği durumlarda,bildiği güvenli liman araçları kullanabilir.
Onun dışında bu mesele içinde 1000 den fazla söyleyecegım var ama uzamasın dıye yazmıyorum.

- [] Subagentlarımızın/multiajanlarımızın başlıca tipleri olmalı;
 - Kaşif
 - Gezgin
 - Anadolu Parsı
 - Baykuş
 - Bal Porsuğu
 - Kartal
 - Kusmuk (upchuck)
 - Juryrigg: Ben10'deki gibi her şeyi debug edebilir,tersıne muhendıslık yapar,her şeyi,her problemı istediği gibi durumun kosullarına uygun sekılde çözer
 - Chamalien: Gizlilik ustasıdır
 - Jetray (Yüzen Kanat)
 - İnsanazor
 - XLR8 (şimşek hız)
 - Fastrack
 - Gölge Hayalet
 - Gri Madde
 - Cannonbolt (yıldırım gülle)
 - Güncelleme
 - Armodrillo
 - Ateş Topu
 - Pul Kanat
 - Blitzwolfer (Baskın Kurt)
 - Yüzen Çene
 - Yaban Köpek
 - Büyük Korku
 - Ditto
 - Echo Echo
 - Vahşi Asma
 - Dört Kol
 - Elmas Kafa
 - Snare-Oh
 - Eye Guy
 - Arctiguana
 - Shocksquatch
 - Rath (Hep sinirli,öfkeli)
 - Fırtına beyin (Brainstorm)
 - Gravattack
 - Atomix
 - Toepick
 - Gutrot
 - Astrodactyl
 - NRG
 - Parlak Taş (Chromastone)
 - Feedback
 - Swampfire (Çamur Ateş)
 - Amfibian (Ampfibian)
 - Waterhazard (Su kampçısı)
 - Terraspin
 - Whampire
 - Ball Weevil (Top böceği)
 - Crashhopper (Çarpan çekige)
 - Bullfrag
 - Clockwork
 - Çoban Yıldızı
 - Örümcek Maymun
 - Goop (Tek Hücre)
 - Charmcaster
 - Blake Blossom
 - Jet Fadıl
 - Nymphomaniac (Nemfomanyak)
 - Rambo
 - The Flash

 Tabi çoğunun görevini yazmadım uzun süreceği için ama onu sonra ayarlarım ben.Fakat gördüğün gibi biraz da tahmın edebilirsin çoğunu.Çizgi filmdeki yeteneklerındne ılham alarak adlarını koydum.
 Tabiki de bu sayı ileride daha da arttırılabilir ki artacakda,çünkü her bir subagentın/multiajanın kendi kişiliği/zekası/ruhu olmasını istiyorum.Yani öyle dümenden hiç bir işe yaramayan köle olmalarını istemiyorum.
 Bu sayede kendi ekosistemimi de kurabilirim,uzun vadede.


- [] Computer Use: Bilgisayar kullanımı üst seviye olmalı.Yani sistemimiz otonom işler yaparken,mcpler yetmeyecektir ve benim kendi bilgisayarımıda otonom şekilde kullaabilmeleri gerekmekte.
Bunun içinde yine gerekli toolları falanda bizim sağlamamız gerekecek.
Piyasadaki mevcut toollarıda kullanabiliriz ama iş akısısımıza gore optımızede edebiliriz,bilmıyopm bakarızo na krar veremödım


- [] Zengin toollarımız olmalı: Örneğin opencodeda nasıl kendi edit/write toolları varsa,bizimde olmalı.Ama onlardaki en büyük eksik bizde olmamalı.Yani llmler kod tabanımızı öğrenmek için kodlarımızı okumamalı mesela,bunu direkt native şekilde çözecek bi toolumuz olmalı örneğin.Yani bunların her problemde sığındıkları tembellık yaptıkları ne kadar araç varsa,hepsını alalım kendımız toola çevirelim,daha iyisini yapalaım,ya da daha ai native toollar yapalım bılemıyorum yani. (hashfile,ffs başlangıç olarak düşünülebilir ama onlarında çok eksiği var.)

Şimdilik özellikler tamam,şimdi de sistem nasıl çalışmalı anlatayım;

- Ajan ister otonom döngüde olsun,ister kullanıcı tabanlı ister başak bir şey,hiç bir şekilde farketmeksizin bu akışı uygulmak zorundadır:
- Eldeki problemi kelime kelime oku -> eldeki problemi küçük parçalara ayır ve problem ne anlatmak istiyor bunu anla (problemler çok çok uzun satırlı olabileceğinden bu kritik.Anlamın kaybolmaması için) -> Problemden anladıklarını bir listeye kaydet (artık nereye kaydeder onu düşünmedimde tartışırız.Kalıcı bi depolama olacak oraya kaydetcek). -> O kaydettiğin listeyi kullanarak,problemi günümüz gerçeği ile internetten derin araştır (web mcpleri aracılığıyla araştıracak.Yüzeysel,derin,okyanus diye üç modu olacak.Modlarına göre daha çok kaynak tarayacak,daha çok nodelara bağlanacak,daha çok yorum/veri okuyacak,daha fazla çalışacak,daha fazla analiz edecek) ->
Araştırma bulgularını json veya artık her ne olursa,hem insan hem de ai'ların native okuyabileceği kaliteli bir formatta depola/sakla (Araştırma sonuçları isteğe bağlı olarak hem video,hem ekran görüntüsü,hem dom işlem kaydı,hem de computer use olarak kaydedilebilir) ->
Kaydettiğin araştırma sonuçlarını oku -> Eğer araştırma sonucu çok büyükse bunu parçalara böl/kaydet,öyle tane tane sindire sindire oku/işle.Büyük değilse direkt işleme devam et -> Problemi günümüz gerçekliği ile anladığın için artık çözüm üretebilirsin.Bunun için uygun yığınları (stack) belirlemelisin. -> Uygun yığınları önce Ai kendisi belirler,ardından sanki kendisi hiç belirlememiş gibi internet neyi öneriyor diye araştırma yapar (yüzeysel veya derin modda).Ardından kendi stack önerisi ile karşılaştırır ve en doğru stacki bulana kadar,çürütme yoluyla bu döngüyü tekrarlar.Nihai stack'e karar verildiğinde ise sonraki aşamaya geçilir. -> Süre belirlenir (Problem MVP olarak mı yoksa tam olarak mı çözülecek,bunun kararı verilir.Bu kararı Ai'ın kendisi vermez.Bunu ayrı bir kontrol mekanizması olan problem süre sistemi karar verir.Ai'ımız bu karara göre hareket eder). -> Plan çıkartılır (yazmıyorum ama yani yukardakilere benzer şekilde işte kod tabanı ogrenme toolumzua sorar veri ögrenır,sonra gerekıyosa yıne arastırma yapar vs vs) -> Plan bir sürü küçük yol taşına/yapı taşına/mahenge dönüştürülür/bölünür/parçalanır. -> Paralel execute sorgulanır (Her bir yapı taşının hangisi paralel yapılabilir,hangisi sırayla yapılmak zorundadır,hangisi olmadan yapılmadan sonrakıne geçileemez gibi vs) -> Sorgulama bittikten sonra ona göre işleme geçilir/multiajan görevlendirmesi yapılır. -> Ajanlar akışı sürdürür (akışı biliyon zatne yazmıyacam işte döngü ilerliyo cart curt s vsvs.Çok uzun amk nıye yazayım sıkerım algorıtmanı). -> Sonra iş başarılya bittikten sonra kullanıcıya telegramdan ya da telefondan sms/çağrı olarak ulaşılır.

Tabi bütün bu süreci kullanıcı ister gerçek zamanlı olarak,ister kayıttan takip de edebilmeli.
İster patlamış mısırını alıop izler,ister izlemez.Fakat yine de kayıtlar her şeyi ile birlikte tutulmalı.Hem yerelde hem bulutta senkronıze tutulmalı 3/2/1 kuralı ile.



Daha aklımda çok şeiy vardıda,aklıma gelmedi.Sen yazdıkca gelir.


KOD YAZMA.

Proje fikrimi detaylı şekilde analiz et.
Eksik gördüğün noktaları ve bilinmeyenleri tespit et.
Bana sorular sorarak gereksinimleri netleştir.

Sonrasında bu çıktıyı başka bir modele verip MASTER-PLAN.md hazırlatacağım.

Bana sadece ALPHA-PLAN.md oluştur.


Not: Şu anda zaten bu istediğimi aynen boyle yazarak bir ürün ortaya çıkarttıkda,tam istediğim gibi olmadı gibi.
İstediğim şeyi anlaymaadılar gibi ve karmancorman bişey lduı gıbı gelıyor,o yuzden once kodları ıncele.
